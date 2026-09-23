use identity::{hash_password, verify_password};

#[test]
fn a_hash_verifies_only_its_own_password() {
    let hash = hash_password("correct horse battery staple");

    assert!(hash.starts_with("$argon2id$"));
    assert!(verify_password("correct horse battery staple", &hash));
    assert!(!verify_password("wrong", &hash));
}

#[test]
fn a_malformed_hash_never_verifies() {
    assert!(!verify_password("anything", "not-a-hash"));
}
