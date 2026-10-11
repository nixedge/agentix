pub async fn index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../static/index.html"))
}
