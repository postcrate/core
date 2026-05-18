pub mod audit;
pub mod bounces;
pub mod chaos;
pub mod emails;
pub mod events;
pub mod health;
pub mod mailboxes;
pub mod mailtrap;
pub mod recording;
pub mod rendering;
pub mod scenarios;
pub mod wait;
pub mod webhooks;

use axum::Router;

use crate::service::ServiceHandle;

pub fn router() -> Router<ServiceHandle> {
    Router::new()
        .merge(health::router())
        .merge(mailtrap::router())
        .nest("/api/v1", api_v1())
}

fn api_v1() -> Router<ServiceHandle> {
    Router::new()
        .merge(mailboxes::router())
        .merge(emails::router())
        .merge(chaos::router())
        .merge(bounces::router())
        .merge(audit::router())
        .merge(wait::router())
        .merge(events::router())
        .merge(recording::router())
        .merge(rendering::router())
        .merge(scenarios::router())
        .merge(webhooks::router())
}
