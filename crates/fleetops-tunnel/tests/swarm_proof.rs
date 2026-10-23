use fleetops_tunnel::control::SwarmClaim;
use fleetops_tunnel::swarm_proof::{
    fingerprint, is_encrypted_key, proof_message, sign, verify, ProofError,
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256,
};

const NOW: i64 = 1_790_000_000;

struct Pki {
    root: String,
    chain: String,
    key: String,
}

fn ca(cn: &str) -> (CertificateParams, KeyPair, String) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name.push(DnType::CommonName, cn);
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let pem = p.self_signed(&key).unwrap().pem();
    (p, key, pem)
}

fn leaf_params(cluster: &str, node: &str, ou: &str) -> CertificateParams {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name.push(DnType::CommonName, node);
    p.distinguished_name.push(DnType::OrganizationName, cluster);
    p.distinguished_name
        .push(DnType::OrganizationalUnitName, ou);
    p
}

/// A swarm root CA and one node certificate (CN=node, O=cluster, OU=ou).
fn swarm_pki(cluster: &str, node: &str, ou: &str) -> Pki {
    let (ca_params, ca_key, root) = ca("swarm-ca");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let chain = leaf_params(cluster, node, ou)
        .signed_by(&key, &issuer)
        .unwrap()
        .pem();
    Pki {
        root,
        chain,
        key: key.serialize_pem(),
    }
}

fn claim(manager: bool) -> SwarmClaim {
    SwarmClaim {
        node_id: "n1".into(),
        cluster_id: "c1".into(),
        manager,
    }
}

fn signed(pki: &Pki, c: &SwarmClaim) -> (Vec<u8>, String) {
    let msg = proof_message(&"ab".repeat(32), c);
    let sig = sign(&pki.key, &msg).unwrap();
    (msg, sig)
}

#[test]
fn a_valid_proof_verifies() {
    let pki = swarm_pki("c1", "n1", "swarm-manager");
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW).unwrap();
}

#[test]
fn rejects_another_ca() {
    let pki = swarm_pki("c1", "n1", "swarm-manager");
    let other = swarm_pki("c1", "n1", "swarm-manager");
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&other.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::BadChain(_))
    ));
}

#[test]
fn rejects_cn_mismatch() {
    let pki = swarm_pki("c1", "n2", "swarm-manager");
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::Mismatch(_))
    ));
}

#[test]
fn rejects_cluster_mismatch() {
    let pki = swarm_pki("c2", "n1", "swarm-manager");
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::Mismatch(_))
    ));
}

#[test]
fn rejects_role_mismatch() {
    let pki = swarm_pki("c1", "n1", "swarm-worker");
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::Mismatch(_))
    ));
    // The same certificate is fine for a worker claim.
    let w = claim(false);
    let (msg, sig) = signed(&pki, &w);
    verify(&pki.root, &pki.chain, &w, &msg, &sig, NOW).unwrap();
}

#[test]
fn rejects_expired() {
    let (ca_params, ca_key, root) = ca("swarm-ca");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = leaf_params("c1", "n1", "swarm-manager");
    p.not_before = rcgen::date_time_ymd(2020, 1, 1);
    p.not_after = rcgen::date_time_ymd(2021, 1, 1);
    let chain = p
        .signed_by(&key, &Issuer::new(ca_params, ca_key))
        .unwrap()
        .pem();
    let pki = Pki {
        root,
        chain,
        key: key.serialize_pem(),
    };
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::Expired)
    ));
}

#[test]
fn rejects_tampered_message() {
    let pki = swarm_pki("c1", "n1", "swarm-manager");
    let c = claim(true);
    let (_, sig) = signed(&pki, &c);
    let other = proof_message(&"cd".repeat(32), &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &other, &sig, NOW),
        Err(ProofError::BadSignature)
    ));
}

#[test]
fn verifies_through_an_intermediate() {
    let (root_params, root_key, root) = ca("swarm-root");
    let mid_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut mid = CertificateParams::new(Vec::<String>::new()).unwrap();
    mid.distinguished_name.push(DnType::CommonName, "swarm-mid");
    mid.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mid_pem = mid
        .signed_by(&mid_key, &Issuer::new(root_params, root_key))
        .unwrap()
        .pem();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let leaf = leaf_params("c1", "n1", "swarm-manager")
        .signed_by(&key, &Issuer::new(mid, mid_key))
        .unwrap()
        .pem();
    let pki = Pki {
        root,
        chain: format!("{leaf}{mid_pem}"),
        key: key.serialize_pem(),
    };
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW).unwrap();
}

#[test]
fn detects_autolocked_key_pem() {
    let pem = "-----BEGIN EC PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nkek-version: 1\nDEK-Info: AES-256-CBC,00\n\nAAAA\n-----END EC PRIVATE KEY-----\n";
    assert!(is_encrypted_key(pem));
    let err = sign(pem, b"x").unwrap_err();
    assert!(matches!(err, ProofError::Autolocked));
    assert_eq!(
        err.to_string(),
        "swarm autolock is on: this manager's node key is encrypted, so FleetOps cannot verify it"
    );
}

#[test]
fn signs_sec1_and_pkcs8_keys() {
    use p256::pkcs8::DecodePrivateKey;
    let pki = swarm_pki("c1", "n1", "swarm-worker");
    let c = claim(false);
    let msg = proof_message(&"ab".repeat(32), &c);
    // rcgen writes PKCS#8; Docker writes SEC1 ("EC PRIVATE KEY").
    let sec1 = p256::SecretKey::from_pkcs8_pem(&pki.key)
        .unwrap()
        .to_sec1_pem(Default::default())
        .unwrap()
        .to_string();
    assert!(!is_encrypted_key(&sec1));
    for key in [&pki.key, &sec1] {
        let sig = sign(key, &msg).unwrap();
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW).unwrap();
    }
}

/// A worker signs a "manager" certificate with its own node key and sends
/// it with its real certificate as the issuer.
#[test]
fn rejects_a_node_cert_used_as_an_issuer() {
    let (ca_params, ca_key, root) = ca("swarm-ca");
    let ca = Issuer::new(ca_params, ca_key);
    let worker_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let worker_params = leaf_params("c1", "w1", "swarm-worker");
    let worker_pem = worker_params.signed_by(&worker_key, &ca).unwrap().pem();
    let forged_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let forged = leaf_params("c1", "n1", "swarm-manager")
        .signed_by(&forged_key, &Issuer::new(worker_params, worker_key))
        .unwrap()
        .pem();
    let pki = Pki {
        root,
        chain: format!("{forged}{worker_pem}"),
        key: forged_key.serialize_pem(),
    };
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::BadChain(_))
    ));
}

#[test]
fn rejects_a_ca_as_the_node_certificate() {
    let (ca_params, ca_key, root) = ca("swarm-ca");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = leaf_params("c1", "n1", "swarm-manager");
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let chain = p
        .signed_by(&key, &Issuer::new(ca_params, ca_key))
        .unwrap()
        .pem();
    let pki = Pki {
        root,
        chain,
        key: key.serialize_pem(),
    };
    let c = claim(true);
    let (msg, sig) = signed(&pki, &c);
    assert!(matches!(
        verify(&pki.root, &pki.chain, &c, &msg, &sig, NOW),
        Err(ProofError::BadChain(_))
    ));
}

#[test]
fn fingerprint_matches_openssl_format() {
    use sha2::{Digest, Sha256};
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name.push(DnType::CommonName, "swarm-ca");
    let cert = p.self_signed(&key).unwrap();
    let want: Vec<String> = Sha256::digest(cert.der())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    assert_eq!(fingerprint(&cert.pem()).unwrap(), want.join(":"));
}
