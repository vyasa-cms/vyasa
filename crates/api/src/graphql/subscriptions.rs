#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! GraphQL subscriptions — live updates via domain event bus.

use async_graphql::{Context, SimpleObject, Subscription};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use vyasa_core::events::{self, Event};

/// Payload for `postUpdated` — clients re-query the post.
#[derive(SimpleObject, Clone, Debug)]
pub struct PostUpdatedPayload {
    /// Updated post id.
    pub post_id: i64,
}

/// Payload for `postPublished`.
#[derive(SimpleObject, Clone, Debug)]
pub struct PostPublishedPayload {
    /// Published post id.
    pub post_id: i64,
    /// Author id.
    pub author_id: i64,
}

/// Subscription root.
pub struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    /// Subscribes to updates for a single post (any `Updated`/`Published`/`Trashed`/`Restored`/`Deleted` for that id).
    /// Emits the post id; clients re-query `post(id)`.
    async fn post_updated(
        &self,
        _ctx: &Context<'_>,
        post_id: i64,
    ) -> async_graphql::Result<impl Stream<Item = PostUpdatedPayload>> {
        let rx = events::subscribe();
        let stream = BroadcastStream::new(rx).filter_map(move |res| {
            let post_id = post_id;
            async move {
                match res {
                    Ok(Event::Updated(e)) if e.post_id == post_id => {
                        Some(PostUpdatedPayload { post_id: e.post_id })
                    }
                    Ok(Event::Published(e)) if e.post_id == post_id => {
                        Some(PostUpdatedPayload { post_id: e.post_id })
                    }
                    Ok(Event::Trashed(e)) if e.post_id == post_id => {
                        Some(PostUpdatedPayload { post_id: e.post_id })
                    }
                    Ok(Event::Restored(e)) if e.post_id == post_id => {
                        Some(PostUpdatedPayload { post_id: e.post_id })
                    }
                    Ok(Event::Deleted(e)) if e.post_id == post_id => {
                        Some(PostUpdatedPayload { post_id: e.post_id })
                    }
                    Ok(Event::CommentAdded(_)) => None,
                    Ok(_) => None,
                    Err(_) => None, // lagged
                }
            }
        });
        Ok(stream)
    }

    /// Subscribes to all `postPublished` events (headless live feed).
    async fn post_published(
        &self,
        _ctx: &Context<'_>,
    ) -> async_graphql::Result<impl Stream<Item = PostPublishedPayload>> {
        let rx = events::subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|res| async move {
            match res {
                Ok(Event::Published(e)) => Some(PostPublishedPayload {
                    post_id: e.post_id,
                    author_id: e.author_id,
                }),
                Ok(_) => None,
                Err(_) => None,
            }
        });
        Ok(stream)
    }
}
