//! A mock identity provider: OIDC discovery, keys and a token endpoint, plus
//! whatever provider APIs a test mounts. Its signing key is a test key.

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

pub const KID: &str = "test-key";
pub const CLIENT_ID: &str = "fleet";
pub const CLIENT_SECRET: &str = "client-secret";

/// PKCS#8, as Google service-account keys are.
pub const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCuA1UzZ1dP/tix
6Enn+ZDozaok3rF5FcitwveTpA4OAD9q+j3+QQtqRjl9grBNiRBoy+Oq5Wne5DDV
IRrJPuelxITW1z0FDAnut9dxj5QcD1zU/Ojxy0DE66QxhnoyPk0Ap4aA00RlzuK3
0NDUOuwlWs1IXWOIcR8jqKD6g9SWPKsDTrnFvtgpx2f/hy+ofLStA7kP71/d4scf
yQGJ/ZtLQg7Rscf0goGQFZAqTa7W0Uv4XDitNfMYopd4erLoHFE5/0Nm4Sr/ZVnJ
Cp5LA+CE6hzme9bMN8AjG8Ppohn46HEEBh7LKH1DNTb3AjJ7wvqBSbIKXsevBuay
LFXX2wGjAgMBAAECggEADPbXAF2abEcahXHYUVd1i79u0qpiSUwgx4yD/dSC7r+z
+dgrVpr2XieQ0IbN2o9aN2ruF+UmbkjmUaqfhtv2tCuGkUVqsjD20hMrEoNvT5ve
MnwMYTjfjXKk2/Yera/+9D9TtQG7npe7dy7JsCLnq9Mum7QuRMcKWkTA7lXa0iQS
3RfjAGeUofdRI8mOb6MPzbQ2Yrsh/eh9sAPIJlqx9ReJzqg9+OuHqHNa3ux+LOe2
QQjyaa3pVZFlq4jzACoeE20XQLsBoKFdRqiy23hiEJ3P7L98vy8Vzu1EgBlc3gKD
acJUdvlhTTKs+VrTkuzWlw3e2mVzWe8C/xdHNvhRYQKBgQDvLzmyJGqpulk2cQl+
Go5sXXZx8rIlzGI4wyM8FEFs+kNTO7/2qzazfrJ4tPbHwfqENSgA4lDwmfXq66mZ
5YcVJdceAvWCkGwCj/G/8mEu+44ew0g7R1/YhNadwFEzd6XqSu6RJF/lOcMTQtke
vRLfZ+0flDQkUfDZv2kj1h6gsQKBgQC6PyuEjDXlLm3U3Qx2UXDwR+AHwmwdYD0j
8+WnvB2/0uSuBvvBxMlzst/G4M3aZKfN4Pa8GIR7wNXswA/ZwFMorqQ5rhYhG2DY
Nndn+r1lEjNPLVSlBAS+hapcUF+1bk1DUFZyynSQWgXsVb4XCkLthWiNX00ZPXDP
/I19aR98kwKBgQCdUt/NV/y8pTD3CUToKEmT56n3613YVOYqVdhANUaPitFr1vo+
IoH5cJWtpcJIxEOmYFxusG3yxCIoIlhNPNcsRtaHDAU2mw4gZVPrIF9iCt6BR4uk
Sh82CLIt0wjWJJYCVMU0vMtCuEqiya6MdGsMfYxP4Cin1ynn3+olQzqkEQKBgGd2
pj85AMuzMjJ5Je4oqfvhgdPtrs/W4cAyQpceyswAZSdEG+F154+NhADga5ryJcIa
QhcV66sv4edmfqexzsCf2/1q90InCnKyNz0dLs+R/8zxWr9c7yHD8XLc9347q1JE
s/GLBJwYVogGzdXQzoNShFVc5uCpoBz6qMNNfb9fAoGAVzIyLNnc9vKtxlKqpZ48
8Wv8+w2vx5vtkJGsGh7aAnhjpnNwsFHoyA++y0UIbR0xNDECFV5ZbE6I09XQaf95
gWHaI7xw8IZpLeB3Y20Nn1fwOJIlMg2ABbcZL26Sro7zce5ZxuNIN/mDy7NwYttS
MzOgE/10YqTNA7irf+cpAjI=
-----END PRIVATE KEY-----";
pub const MODULUS: &str = "rgNVM2dXT_7YsehJ5_mQ6M2qJN6xeRXIrcL3k6QODgA_avo9_kELakY5fYKwTYkQaMvjquVp3uQw1SEayT7npcSE1tc9BQwJ7rfXcY-UHA9c1Pzo8ctAxOukMYZ6Mj5NAKeGgNNEZc7it9DQ1DrsJVrNSF1jiHEfI6ig-oPUljyrA065xb7YKcdn_4cvqHy0rQO5D-9f3eLHH8kBif2bS0IO0bHH9IKBkBWQKk2u1tFL-Fw4rTXzGKKXeHqy6BxROf9DZuEq_2VZyQqeSwPghOoc5nvWzDfAIxvD6aIZ-OhxBAYeyyh9QzU29wIye8L6gUmyCl7HrwbmsixV19sBow";

pub struct Idp {
    pub server: MockServer,
}

impl Idp {
    /// Serves discovery for `issuer_path` ("" for the root).
    pub async fn start(issuer_path: &str) -> Idp {
        let server = MockServer::start().await;
        let issuer = format!("{}{issuer_path}", server.uri());
        Mock::given(method("GET"))
            .and(path(format!(
                "{issuer_path}/.well-known/openid-configuration"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "jwks_uri": format!("{}/jwks", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "keys": [{"kty": "RSA", "kid": KID, "alg": "RS256", "use": "sig", "n": MODULUS, "e": "AQAB"}]
            })))
            .mount(&server)
            .await;
        Idp { server }
    }

    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// An ID token signed with the test key (or `key`), with `claims`
    /// over sensible defaults for `issuer`.
    pub fn id_token(&self, issuer: &str, nonce: &str, claims: Value) -> String {
        sign(&self.base_claims(issuer, nonce, claims), KID)
    }

    pub fn base_claims(&self, issuer: &str, nonce: &str, claims: Value) -> Value {
        let now = fleetops_core::now_unix();
        let mut c = json!({
            "iss": issuer, "aud": CLIENT_ID, "sub": "user-1", "nonce": nonce,
            "iat": now, "exp": now + 300, "email": "jane@acme.io",
        });
        for (k, v) in claims.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    /// The token endpoint answers with `id_token` (and an access token).
    pub async fn answer_token(&self, id_token: Option<String>) {
        let mut body =
            json!({"access_token": "access-1", "token_type": "Bearer", "expires_in": 300});
        if let Some(t) = id_token {
            body["id_token"] = json!(t);
        }
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// GET `p` answers `body`.
    pub async fn json(&self, p: &str, body: Value) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Any method on `p` answers `status`.
    pub async fn status(&self, p: &str, status: u16) {
        Mock::given(path(p))
            .respond_with(ResponseTemplate::new(status))
            .mount(&self.server)
            .await;
    }
}

/// Signs `claims` with the test key, under key id `kid`.
pub fn sign(claims: &Value, kid: &str) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some(kid.into());
    jsonwebtoken::encode(
        &h,
        claims,
        &EncodingKey::from_rsa_pem(PRIVATE_KEY.as_bytes()).unwrap(),
    )
    .unwrap()
}
