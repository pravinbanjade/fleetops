//! Puts the plugin's token on every request (spec 3k §2). A 401 forgets it
//! so the next request runs the plugin again.

use super::TokenSource;
use futures_util::future::BoxFuture;
use hyper::{header::AUTHORIZATION, http::HeaderValue, Request, Response, StatusCode};
use kube::client::Body;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{BoxError, Layer, Service, ServiceExt};

#[derive(Clone)]
pub struct ExecAuthLayer(pub Arc<TokenSource>);

impl<S> Layer<S> for ExecAuthLayer {
    type Service = ExecAuth<S>;
    fn layer(&self, inner: S) -> ExecAuth<S> {
        ExecAuth {
            inner: Arc::new(tokio::sync::Mutex::new(inner)),
            source: self.0.clone(),
        }
    }
}

/// kube-rs's service stack is a `BoxService`, which is not `Clone`, so the
/// inner service sits behind a mutex held only while a call starts.
pub struct ExecAuth<S> {
    inner: Arc<tokio::sync::Mutex<S>>,
    source: Arc<TokenSource>,
}

impl<S> Clone for ExecAuth<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            source: self.source.clone(),
        }
    }
}

impl<S, B> Service<Request<Body>> for ExecAuth<S>
where
    S: Service<Request<Body>, Response = Response<B>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
    B: Send + 'static,
{
    type Response = Response<B>;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Response<B>, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        // Readiness is checked under the lock in `call`.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        let source = self.source.clone();
        Box::pin(async move {
            let token = source.token().await.map_err(|e| Box::new(e) as BoxError)?;
            let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                Box::new(super::ExecError::NoToken(source.command().into())) as BoxError
            })?;
            value.set_sensitive(true);
            req.headers_mut().insert(AUTHORIZATION, value);
            let fut = {
                let mut s = inner.lock().await;
                s.ready().await.map_err(Into::into)?;
                s.call(req)
            };
            let resp = fut.await.map_err(Into::into)?;
            if resp.status() == StatusCode::UNAUTHORIZED {
                source.invalidate().await;
            }
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec_auth::{provider::Runner, ExecError, Plugin};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(AtomicUsize);
    #[async_trait]
    impl Runner for Counting {
        async fn run(&self, _: &Plugin) -> Result<Vec<u8>, ExecError> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(format!("{{\"status\":{{\"token\":\"t{n}\"}}}}").into_bytes())
        }
    }

    fn source() -> (Arc<TokenSource>, Arc<Counting>) {
        let r = Arc::new(Counting(AtomicUsize::new(0)));
        let p = Plugin {
            command: "x".into(),
            args: vec![],
            env: vec![],
            exec_info: "{}".into(),
        };
        (Arc::new(TokenSource::new(p, r.clone())), r)
    }

    #[tokio::test]
    async fn every_request_carries_the_token_and_a_401_refreshes_it() {
        let (src, runs) = source();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let inner = tower::service_fn(move |req: Request<Body>| {
            let s2 = s2.clone();
            async move {
                let auth = req.headers()[AUTHORIZATION].to_str().unwrap().to_string();
                let status = if s2.lock().unwrap().is_empty() {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::OK
                };
                s2.lock().unwrap().push(auth);
                Ok::<_, BoxError>(Response::builder().status(status).body(()).unwrap())
            }
        });
        let mut svc = ExecAuthLayer(src).layer(inner);
        let req = || Request::get("/version").body(Body::empty()).unwrap();
        let r = svc.ready().await.unwrap().call(req()).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        svc.ready().await.unwrap().call(req()).await.unwrap();
        svc.ready().await.unwrap().call(req()).await.unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            ["Bearer t0", "Bearer t1", "Bearer t1"]
        );
        assert_eq!(runs.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_plugin_failure_fails_the_request_with_its_message() {
        struct Missing;
        #[async_trait]
        impl Runner for Missing {
            async fn run(&self, _: &Plugin) -> Result<Vec<u8>, ExecError> {
                Err(ExecError::NotFound("aws".into()))
            }
        }
        let p = Plugin {
            command: "aws".into(),
            args: vec![],
            env: vec![],
            exec_info: "{}".into(),
        };
        let src = Arc::new(TokenSource::new(p, Arc::new(Missing)));
        let inner =
            tower::service_fn(|_req: Request<Body>| async { Ok::<_, BoxError>(Response::new(())) });
        let mut svc = ExecAuthLayer(src).layer(inner);
        let e = svc
            .ready()
            .await
            .unwrap()
            .call(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "exec plugin 'aws' was not found on the server's PATH"
        );
    }
}
