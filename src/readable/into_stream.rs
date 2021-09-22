use core::pin::Pin;

use futures::future::FutureExt;
use futures::ready;
use futures::stream::{FusedStream, Stream};
use futures::task::{Context, Poll};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::sys::ReadableStreamReadResult;
use super::ReadableStreamDefaultReader;

/// A [`Stream`](futures::Stream) for the [`into_stream`](super::ReadableStream::into_stream) method.
///
/// This `Stream` holds a reader, and therefore locks the [`ReadableStream`](super::ReadableStream).
/// When this `Stream` is dropped, it also drops its reader which in turn
/// [releases its lock](https://streams.spec.whatwg.org/#release-a-lock).
///
/// When used through [`ReadableStream::into_stream`](super::ReadableStream::into_stream),
/// the stream is automatically cancelled before dropping the reader, discarding any pending read requests.
/// When used through [`ReadableStreamDefaultReader::into_stream`](super::ReadableStreamDefaultReader::into_stream),
/// it is up to the user to either manually [cancel](Self::cancel) the stream,
/// or to ensure that there are no pending read requests when dropped.
/// See the documentation on [`ReadableStreamDefaultReader`] for more details on the drop behavior.
#[must_use = "streams do nothing unless polled"]
#[derive(Debug)]
pub struct IntoStream<'reader> {
    reader: Option<ReadableStreamDefaultReader<'reader>>,
    fut: Option<JsFuture>,
    cancel_on_drop: bool,
}

impl<'reader> IntoStream<'reader> {
    #[inline]
    pub(super) fn new(reader: ReadableStreamDefaultReader, cancel_on_drop: bool) -> IntoStream {
        IntoStream {
            reader: Some(reader),
            fut: None,
            cancel_on_drop,
        }
    }

    /// [Cancels](https://streams.spec.whatwg.org/#cancel-a-readable-stream) the stream,
    /// signaling a loss of interest in the stream by a consumer.
    pub async fn cancel(mut self) -> Result<(), JsValue> {
        match self.reader.take() {
            Some(mut reader) => reader.cancel().await,
            None => Ok(()),
        }
    }

    /// [Cancels](https://streams.spec.whatwg.org/#cancel-a-readable-stream) the stream,
    /// signaling a loss of interest in the stream by a consumer.
    pub async fn cancel_with_reason(mut self, reason: &JsValue) -> Result<(), JsValue> {
        match self.reader.take() {
            Some(mut reader) => reader.cancel_with_reason(reason).await,
            None => Ok(()),
        }
    }
}

impl FusedStream for IntoStream<'_> {
    fn is_terminated(&self) -> bool {
        self.reader.is_none() && self.fut.is_none()
    }
}

impl<'reader> Stream for IntoStream<'reader> {
    type Item = Result<JsValue, JsValue>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.fut.is_none() {
            // No pending read, start reading the next chunk
            match &self.reader {
                Some(reader) => {
                    // Read a chunk and store its future
                    let fut = JsFuture::from(reader.as_raw().read());
                    self.fut = Some(fut);
                }
                None => {
                    // Reader was already dropped
                    return Poll::Ready(None);
                }
            }
        }

        // Poll the future for the pending read
        let js_result = ready!(self.as_mut().fut.as_mut().unwrap_throw().poll_unpin(cx));
        self.fut = None;

        // Read completed
        Poll::Ready(match js_result {
            Ok(js_value) => {
                let result = ReadableStreamReadResult::from(js_value);
                if result.is_done() {
                    // End of stream, drop reader
                    self.reader = None;
                    None
                } else {
                    Some(Ok(result.value()))
                }
            }
            Err(js_value) => {
                // Error, drop reader
                self.reader = None;
                Some(Err(js_value))
            }
        })
    }
}

impl<'reader> Drop for IntoStream<'reader> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            if let Some(reader) = self.reader.take() {
                let _ = reader.as_raw().cancel().catch(&Closure::once(|_| {}));
            }
        }
    }
}
