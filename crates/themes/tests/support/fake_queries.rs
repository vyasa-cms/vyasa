//! Minimal `ContentQueries` stub for end-to-end render tests.
//!
//! Shared by every integration test that calls `render_page`; the trait has
//! no default methods, so without this each one would carry the same eighty
//! lines of `Ok(vec![])`.
use std::future::ready;

pub struct FakeQueries;

impl FakeQueries {
    pub fn shared() -> &'static FakeQueries {
        static Q: FakeQueries = FakeQueries;
        &Q
    }
}

fn empty<T: Send + 'static>() -> Vec<T> {
    Vec::new()
}

impl vyasa_themes::ContentQueries for FakeQueries {
    fn entries<'a>(
        &'a self,
        _source: &'a str,
        _sort: vyasa_themes::EntrySort,
        _limit: u32,
        _term: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::PostCardData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(ready(Ok(empty())))
    }
    fn categories<'a>(
        &'a self,
        _show_counts: bool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(ready(Ok(empty())))
    }
    fn popular_tags<'a>(
        &'a self,
        _count: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(ready(Ok(empty())))
    }
    fn monthly_archives<'a>(
        &'a self,
        _months: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::MonthArchiveData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(ready(Ok(empty())))
    }
    fn navigation_menu<'a>(
        &'a self,
        _slug: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(ready(Ok(String::new())))
    }
    fn page_tree<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::PageNodeData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
    fn approved_comments<'a>(
        &'a self,
        _post_id: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::CommentNodeData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(ready(Ok(empty())))
    }
}
