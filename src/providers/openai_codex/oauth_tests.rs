use super::*;
use std::sync::Mutex as StdMutex;
use wiremock::matchers::{body_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug, Default)]
struct LoginStore {
    stored_expiry: StdMutex<Option<i64>>,
}

impl SecretStore for LoginStore {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<crate::core::secrets::ResolvedCredential, SecretError> {
        Err(SecretError::NotFound {
            provider: provider.clone(),
        })
    }

    fn store(&self, provider: &ProviderId, credential: Credential) -> Result<(), SecretError> {
        assert_eq!(provider.as_str(), super::super::PROVIDER_ID);
        let oauth = credential.into_oauth().expect("OAuth credential");
        assert_eq!(oauth.access_token().expose(), FRESH_ACCESS);
        assert_eq!(oauth.refresh_token().expose(), "refresh-new");
        assert_eq!(oauth.account_id(), "account-1");
        *self.stored_expiry.lock().expect("expiry lock") = Some(oauth.expires_at_unix());
        Ok(())
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Ok(())
    }
}

const FRESH_ACCESS: &str =
    "e30.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2NvdW50LTEiLCJleHAiOjQxMDI0NDQ4MDB9.sig";

#[test]
fn jwt_claims_support_top_level_and_nested_account_ids() {
    let top = "e30.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2NvdW50LTEiLCJleHAiOjE3MDAwMDAwMDB9.sig";
    let nested = "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjb3VudC0yIn0sImV4cCI6MTcwMDAwMDAwMX0.sig";

    let top_claims = token_claims(top).expect("top-level claims");
    let nested_claims = token_claims(nested).expect("nested claims");
    assert_eq!(top_claims.account_id.as_deref(), Some("account-1"));
    assert_eq!(top_claims.expires_at_unix, Some(1_700_000_000));
    assert_eq!(nested_claims.account_id.as_deref(), Some("account-2"));
    assert_eq!(nested_claims.expires_at_unix, Some(1_700_000_001));
}

#[test]
fn invalid_jwt_payloads_fail_without_echoing_the_token() {
    let token = "header.not-valid!.signature-secret";
    let error = token_claims(token).expect_err("invalid token must fail");
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains("signature-secret"));
    assert!(!rendered.contains("not-valid"));
}

#[test]
fn oauth_form_encoding_preserves_rotating_token_bytes() {
    assert_eq!(
        form_body(&[("refresh_token", "a+b/c=d e")]),
        "refresh_token=a%2Bb%2Fc%3Dd+e"
    );
}

#[tokio::test]
async fn device_login_uses_the_official_sequence_and_persists_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/usercode"))
        .and(body_json(serde_json::json!({ "client_id": CLIENT_ID })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_auth_id": "device-1",
            "user_code": "ABCD-EFGH",
            "interval": "0"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .and(body_json(serde_json::json!({
            "device_auth_id": "device-1",
            "user_code": "ABCD-EFGH"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "authorization_code": "authorization-1",
            "code_challenge": "challenge-1",
            "code_verifier": "verifier-1"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=authorization-1"))
        .and(body_string_contains("code_verifier=verifier-1"))
        .and(body_string_contains(format!("client_id={CLIENT_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": FRESH_ACCESS,
            "refresh_token": "refresh-new",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(LoginStore::default());
    let prompt = Arc::new(StdMutex::new(None));
    let prompt_for_callback = Arc::clone(&prompt);

    let expiry = device_login_at(
        reqwest::Client::new(),
        OAuthEndpoints::for_test(server.uri()),
        Arc::clone(&store) as Arc<dyn SecretStore>,
        move |announced| {
            *prompt_for_callback.lock().expect("prompt lock") = Some(announced);
        },
    )
    .await
    .expect("device login");

    assert_eq!(expiry, 4_102_444_800);
    assert_eq!(
        store.stored_expiry.lock().expect("expiry lock").as_ref(),
        Some(&4_102_444_800)
    );
    assert_eq!(
        prompt.lock().expect("prompt lock").as_ref(),
        Some(&DevicePrompt {
            verification_url: format!("{}/codex/device", server.uri()),
            user_code: "ABCD-EFGH".to_owned(),
        })
    );
}
