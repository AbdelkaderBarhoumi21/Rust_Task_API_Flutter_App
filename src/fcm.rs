use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use yup_oauth2::{
    authenticator::Authenticator, hyper::client::HttpConnector, hyper_rustls::HttpsConnector,
    ServiceAccountAuthenticator, ServiceAccountKey,
};

const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

#[derive(Debug, thiserror::Error)]
pub enum FcmError {
    #[error("oauth error: {0}")]
    Oauth(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("fcm api error {status}: {body}")]
    Api { status: u16, body: String },
}

pub struct FcmClient {
    project_id: String,
    http: reqwest::Client,
    authenticator: Authenticator<HttpsConnector<HttpConnector>>,
    cached_token: RwLock<Option<(String, std::time::Instant)>>,
}

impl FcmClient {
    pub async fn new(project_id: String, service_account_path: &str) -> anyhow::Result<Arc<Self>> {
        let key: ServiceAccountKey = yup_oauth2::read_service_account_key(service_account_path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read service account: {e}"))?;

        let authenticator = ServiceAccountAuthenticator::builder(key)
            .build()
            .await
            .map_err(|e| anyhow::anyhow!("failed to build authenticator: {e}"))?;

        Ok(Arc::new(Self {
            project_id,
            http: reqwest::Client::new(),
            authenticator,
            cached_token: RwLock::new(None),
        }))
    }

    async fn access_token(&self) -> Result<String, FcmError> {
        {
            let guard = self.cached_token.read().await;
            if let Some((token, expires_at)) = guard.as_ref() {
                if std::time::Instant::now() < *expires_at {
                    return Ok(token.clone());
                }
            }
        }

        let token = self
            .authenticator
            .token(&[FCM_SCOPE])
            .await
            .map_err(|e| FcmError::Oauth(e.to_string()))?;
        let token_str = token
            .token()
            .ok_or_else(|| FcmError::Oauth("empty token".into()))?
            .to_string();

        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(50 * 60);
        *self.cached_token.write().await = Some((token_str.clone(), expires_at));
        Ok(token_str)
    }

    pub async fn send_to_token(
        &self,
        device_token: &str,
        title: &str,
        body: &str,
        data: Option<Value>,
    ) -> Result<(), FcmError> {
        let token = self.access_token().await?;
        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        );

        let payload = json!({
            "message": {
                "token": device_token,
                "notification": { "title": title, "body": body },
                "data": data.unwrap_or(json!({})),
                "android": {
                    "priority": "HIGH",
                    "notification": {
                        "channel_id": "task_updates",
                        "icon": "ic_notification"
                    }
                },
                "apns": {
                    "payload": { "aps": { "sound": "default" } }
                }
            }
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(FcmError::Api { status, body });
        }
        Ok(())
    }
}
