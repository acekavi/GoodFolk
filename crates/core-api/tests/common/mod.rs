#![allow(dead_code)] // each test binary uses a different subset

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use core_api::{AppState, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use tower::ServiceExt;

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub pool: PgPool,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl TestApp {
    pub async fn new(opts: PgConnectOptions) -> Self {
        Self::with_pool(db::testing::app_pool(opts, 5).await)
    }

    pub fn with_pool(pool: PgPool) -> Self {
        let state = AppState::new(pool.clone(), false, db::testing::guest_id_key());
        Self { router: router(state.clone()), state, pool }
    }

    /// Sends a request as the browser SPA would: JSON, with the CSRF header.
    pub async fn send(&self, method: Method, path: &str, cookie: Option<&str>, body: Option<Value>) -> TestResponse {
        self.send_with(method, path, cookie, body, &[("x-goodfolk-csrf", "1")]).await
    }

    pub async fn send_with(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> TestResponse {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let request = match body {
            Some(body) => builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .unwrap();
        self.call(request).await
    }

    /// Sends a raw body, e.g. one that is not valid JSON, with the CSRF header.
    pub async fn send_raw(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        content_type: Option<&str>,
        body: &str,
    ) -> TestResponse {
        let mut builder = Request::builder().method(method).uri(path).header("x-goodfolk-csrf", "1");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        self.call(builder.body(Body::from(body.to_owned())).unwrap()).await
    }

    /// A body that is not JSON comes back as a JSON string, so a test can show what it was.
    async fn call(&self, request: Request<Body>) -> TestResponse {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        TestResponse { status, headers, body }
    }

    /// Signs up a new owner and returns their session cookie (`name=value`).
    pub async fn signup_owner(&self, email: &str, tenant_name: &str) -> String {
        let response = self
            .send(
                Method::POST,
                "/api/v1/auth/signup",
                None,
                Some(json!({
                    "email": email,
                    "password": "a long enough password",
                    "display_name": "Owner",
                    "tenant_name": tenant_name,
                })),
            )
            .await;
        assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
        session_cookie(&response.headers)
    }
}

impl TestApp {
    /// Signs up `email` and makes them `role` in the tenant of `owner_cookie`, with their session switched to it.
    /// Staff invitations arrive in Phase 8; until then the membership is written directly as `superuser`.
    pub async fn staff(&self, superuser: &PgPool, owner_cookie: &str, email: &str, role: &str) -> String {
        let cookie = self.signup_owner(email, "Own tenant").await;
        let user = self.send(Method::GET, "/api/v1/me", Some(&cookie), None).await.body["user_id"].clone();
        let tenant =
            self.send(Method::GET, "/api/v1/me", Some(owner_cookie), None).await.body["current_tenant"].clone();
        let (user, tenant) = (uuid(&user), uuid(&tenant));
        sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
            .bind(tenant)
            .bind(user)
            .execute(superuser)
            .await
            .unwrap();
        sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, $4)")
            .bind(uuid::Uuid::now_v7())
            .bind(tenant)
            .bind(user)
            .bind(role)
            .execute(superuser)
            .await
            .unwrap();
        let switched =
            self.send(Method::PUT, "/api/v1/session/tenant", Some(&cookie), Some(json!({"tenant_id": tenant}))).await;
        assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);
        cookie
    }
}

pub fn uuid(value: &Value) -> uuid::Uuid {
    uuid::Uuid::parse_str(value.as_str().expect("a UUID string")).unwrap()
}

pub fn session_cookie(headers: &HeaderMap) -> String {
    let set_cookie = headers.get(header::SET_COOKIE).expect("Set-Cookie header").to_str().unwrap();
    set_cookie.split(';').next().unwrap().to_owned()
}
