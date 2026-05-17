pub mod bounces;
pub mod chaos;
pub mod emails;
pub mod health;
pub mod mailboxes;

use axum::Router;

use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .merge(health::router())
        .nest("/api/v1", api_v1())
}

fn api_v1() -> Router<ServiceHandle> {
    Router::new()
        .merge(mailboxes::router())
        .merge(emails::router())
        .merge(chaos::router())
        .merge(bounces::router())
}
