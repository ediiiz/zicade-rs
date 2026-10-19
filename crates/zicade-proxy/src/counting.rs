//! A body wrapper that reports each data frame's size to the connection's
//! [`Tracked`] as the frame passes, so plain-HTTP bodies show up in live
//! throughput just like tunnel bytes.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};

use crate::http_forward::BoxedBody;
use crate::tracker::Tracked;

/// Which way the wrapped body flows.
#[derive(Clone, Copy)]
pub(crate) enum Flow {
    /// A request body, client -> origin.
    Up,
    /// A response body, origin -> client.
    Down,
}

struct Counting {
    inner: BoxedBody,
    tracked: Arc<Tracked>,
    flow: Flow,
}

/// Wrap `body`, counting its bytes against `tracked`. The wrapper also keeps
/// the connection's entry alive until the body is finished or dropped.
pub(crate) fn counting(body: BoxedBody, tracked: Arc<Tracked>, flow: Flow) -> BoxedBody {
    Counting {
        inner: body,
        tracked,
        flow,
    }
    .boxed()
}

impl Body for Counting {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(data) = frame.data_ref()
        {
            let n = data.len() as u64;
            match self.flow {
                Flow::Up => self.tracked.add_up(n),
                Flow::Down => self.tracked.add_down(n),
            }
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
