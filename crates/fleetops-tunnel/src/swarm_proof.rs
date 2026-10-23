//! Proof that a Swarm agent runs on the node it claims (swarm spec §2). The
//! agent signs a server nonce with the node's Swarm key; the server checks
//! the node certificate against the swarm's root CA and its subject
//! (CN = node id, O = cluster id, OU = role) against the claim.

use crate::control::SwarmClaim;
use base64::{engine::general_purpose::STANDARD, Engine};
use p256::ecdsa::{signature::Signer, signature::Verifier, Signature, SigningKey, VerifyingKey};
use x509_parser::{certificate::X509Certificate, pem::Pem, time::ASN1Time};

#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error(
        "swarm autolock is on: this manager's node key is encrypted, so FleetOps cannot verify it"
    )]
    Autolocked,
    #[error("unusable node key: {0}")]
    BadKey(String),
    #[error("node certificate does not chain to the swarm's root CA: {0}")]
    BadChain(String),
    #[error("node certificate has expired or is not yet valid")]
    Expired,
    #[error("node certificate does not match the claim: {0}")]
    Mismatch(String),
    #[error("bad node signature")]
    BadSignature,
}

pub fn proof_message(nonce: &str, claim: &SwarmClaim) -> Vec<u8> {
    format!(
        "fleetops-swarm-proof/v1\n{nonce}\n{}\n{}",
        claim.node_id, claim.cluster_id
    )
    .into_bytes()
}

/// Swarm encrypts manager keys when autolock is on.
pub fn is_encrypted_key(key_pem: &str) -> bool {
    key_pem.contains("Proc-Type: 4,ENCRYPTED") || key_pem.contains("kek-version")
}

/// Signs `msg` with a P-256 key in SEC1 (as Docker writes it) or PKCS#8
/// PEM; returns base64 DER.
pub fn sign(key_pem: &str, msg: &[u8]) -> Result<String, ProofError> {
    use p256::pkcs8::DecodePrivateKey;
    if is_encrypted_key(key_pem) {
        return Err(ProofError::Autolocked);
    }
    let secret = p256::SecretKey::from_sec1_pem(key_pem)
        .or_else(|_| p256::SecretKey::from_pkcs8_pem(key_pem))
        .map_err(|e| ProofError::BadKey(e.to_string()))?;
    let sig: Signature = SigningKey::from(secret).sign(msg);
    Ok(STANDARD.encode(sig.to_der().as_bytes()))
}

fn pems(pem: &str) -> Result<Vec<Pem>, ProofError> {
    let all: Vec<Pem> = Pem::iter_from_buffer(pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|e| ProofError::BadChain(e.to_string()))?;
    let certs: Vec<Pem> = all
        .into_iter()
        .filter(|p| p.label == "CERTIFICATE")
        .collect();
    if certs.is_empty() {
        return Err(ProofError::BadChain("no certificate".into()));
    }
    Ok(certs)
}

/// Swarm's own CA signs node certificates directly; an external CA may
/// add an intermediate or two.
const MAX_CHAIN: usize = 4;

/// A CA by basicConstraints, allowed to sign certificates by keyUsage when
/// it has one.
fn is_ca(cert: &X509Certificate) -> bool {
    let ca = cert
        .basic_constraints()
        .ok()
        .flatten()
        .is_some_and(|b| b.value.ca);
    let signs = match cert.key_usage() {
        Ok(Some(ku)) => ku.value.key_cert_sign(),
        Ok(None) => true,
        Err(_) => false,
    };
    ca && signs
}

fn parse(p: &Pem) -> Result<X509Certificate<'_>, ProofError> {
    p.parse_x509()
        .map_err(|e| ProofError::BadChain(e.to_string()))
}

fn attr<'a>(
    mut it: impl Iterator<Item = &'a x509_parser::x509::AttributeTypeAndValue<'a>>,
) -> Option<String> {
    it.next().and_then(|a| a.as_str().ok()).map(str::to_string)
}

/// Checks the chain, the subject and the signature; `now_unix` is the time
/// the leaf must be valid at.
pub fn verify(
    root_pem: &str,
    chain_pem: &str,
    claim: &SwarmClaim,
    msg: &[u8],
    signature: &str,
    now_unix: i64,
) -> Result<(), ProofError> {
    let roots = pems(root_pem)?;
    let chain = pems(chain_pem)?;
    if chain.len() > MAX_CHAIN {
        return Err(ProofError::BadChain(format!(
            "{} certificates; at most {MAX_CHAIN}",
            chain.len()
        )));
    }
    let root = parse(&roots[0])?;
    let certs: Vec<X509Certificate> = chain.iter().map(parse).collect::<Result<_, _>>()?;
    let now =
        ASN1Time::from_timestamp(now_unix).map_err(|e| ProofError::BadChain(e.to_string()))?;

    // Each certificate is signed by the next, the last by the root, and
    // every issuer must be a CA: otherwise a node could sign a certificate
    // for any other node with its own node key.
    for (i, cert) in certs.iter().enumerate() {
        let issuer = certs.get(i + 1).unwrap_or(&root);
        cert.verify_signature(Some(issuer.public_key()))
            .map_err(|e| ProofError::BadChain(e.to_string()))?;
        if cert.issuer().as_raw() != issuer.subject().as_raw() {
            return Err(ProofError::BadChain(format!(
                "certificate {i} names an issuer other than the certificate after it"
            )));
        }
        if !is_ca(issuer) {
            return Err(ProofError::BadChain(format!(
                "certificate {i} is signed by a certificate that is not a CA"
            )));
        }
        if i > 0 && !cert.validity().is_valid_at(now) {
            return Err(ProofError::BadChain(format!(
                "intermediate certificate {i} has expired or is not yet valid"
            )));
        }
    }
    let leaf = &certs[0];
    if leaf
        .basic_constraints()
        .ok()
        .flatten()
        .is_some_and(|b| b.value.ca)
    {
        return Err(ProofError::BadChain("the node certificate is a CA".into()));
    }
    if !leaf.validity().is_valid_at(now) {
        return Err(ProofError::Expired);
    }

    let subject = leaf.subject();
    let cn = attr(subject.iter_common_name()).unwrap_or_default();
    let o = attr(subject.iter_organization()).unwrap_or_default();
    let ou = attr(subject.iter_organizational_unit()).unwrap_or_default();
    if cn != claim.node_id {
        return Err(ProofError::Mismatch(format!(
            "certificate is for node {cn}, agent claims {}",
            claim.node_id
        )));
    }
    if o != claim.cluster_id {
        return Err(ProofError::Mismatch(format!(
            "certificate is for swarm {o}, agent claims {}",
            claim.cluster_id
        )));
    }
    let expected = if claim.manager {
        "swarm-manager"
    } else {
        "swarm-worker"
    };
    if ou != expected {
        return Err(ProofError::Mismatch(format!(
            "certificate role is {ou}, agent claims {expected}"
        )));
    }

    let key = VerifyingKey::from_sec1_bytes(&leaf.public_key().subject_public_key.data)
        .map_err(|e| ProofError::BadChain(format!("node key: {e}")))?;
    let der = STANDARD
        .decode(signature)
        .map_err(|_| ProofError::BadSignature)?;
    let sig = Signature::from_der(&der).map_err(|_| ProofError::BadSignature)?;
    key.verify(msg, &sig).map_err(|_| ProofError::BadSignature)
}
