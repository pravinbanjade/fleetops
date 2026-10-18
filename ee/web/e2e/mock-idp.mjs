// A tiny OpenID Connect provider for the Enterprise e2e suite: it signs in
// one fixed user (groups ["ops"]) without asking, the way a provider does
// for someone already logged in there. Never use it outside tests.
import { createHash, createSign, generateKeyPairSync, randomBytes } from "node:crypto";
import { createServer } from "node:http";

const port = Number(process.env.MOCK_IDP_PORT ?? 9124);
const issuer = `http://127.0.0.1:${port}`;
const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "e2e", alg: "RS256", use: "sig" };
const USER = { sub: "e2e-sso-1", preferred_username: "sso-jane", email: "jane@acme.test", groups: ["ops"] };

/** code → what the token endpoint needs to answer it. */
const codes = new Map();

const b64 = (v) => Buffer.from(typeof v === "string" ? v : JSON.stringify(v)).toString("base64url");

function idToken(clientId, nonce) {
  const now = Math.floor(Date.now() / 1000);
  const body = `${b64({ alg: "RS256", typ: "JWT", kid: "e2e" })}.${b64({ ...USER, iss: issuer, aud: clientId, nonce, iat: now, exp: now + 300 })}`;
  const sig = createSign("RSA-SHA256").update(body).sign(privateKey).toString("base64url");
  return `${body}.${sig}`;
}

function json(res, status, v) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(v));
}

createServer((req, res) => {
  const url = new URL(req.url, issuer);
  if (url.pathname === "/.well-known/openid-configuration") {
    return json(res, 200, {
      issuer,
      authorization_endpoint: `${issuer}/authorize`,
      token_endpoint: `${issuer}/token`,
      jwks_uri: `${issuer}/jwks`,
    });
  }
  if (url.pathname === "/jwks") return json(res, 200, { keys: [jwk] });
  if (url.pathname === "/authorize") {
    const q = url.searchParams;
    const code = randomBytes(16).toString("hex");
    codes.set(code, { nonce: q.get("nonce"), clientId: q.get("client_id"), challenge: q.get("code_challenge") });
    const back = new URL(q.get("redirect_uri"));
    back.searchParams.set("code", code);
    back.searchParams.set("state", q.get("state"));
    res.writeHead(302, { location: back.toString() });
    return res.end();
  }
  if (url.pathname === "/token" && req.method === "POST") {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const form = new URLSearchParams(raw);
      const grant = codes.get(form.get("code"));
      codes.delete(form.get("code"));
      const verifier = form.get("code_verifier") ?? "";
      if (!grant || createHash("sha256").update(verifier).digest("base64url") !== grant.challenge) {
        return json(res, 400, { error: "invalid_grant" });
      }
      json(res, 200, { access_token: "e2e-access", token_type: "Bearer", expires_in: 300, id_token: idToken(grant.clientId, grant.nonce) });
    });
    return;
  }
  if (url.pathname === "/health") return json(res, 200, { ok: true });
  json(res, 404, { error: "not_found" });
}).listen(port, "127.0.0.1");
