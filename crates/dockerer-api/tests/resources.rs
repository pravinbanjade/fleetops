mod common;

use common::*;

#[tokio::test]
async fn system_info_and_df() {
    let app = app().await;
    let s = app.with_local().await;
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/info", None, Some(&s)))
            .await
            .json["NCPU"],
        2
    );
    assert!(app
        .send(req("GET", "/api/v1/envs/local/df", None, Some(&s)))
        .await
        .json
        .is_object());
}
