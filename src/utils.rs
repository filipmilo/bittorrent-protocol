use sha1::{Digest, Sha1};

pub fn sha1(candidate: &Vec<u8>) -> (Vec<u8>, String) {
    let mut hasher = Sha1::new();

    hasher.update(candidate);

    let result = hasher.finalize();

    (
        result.iter().map(|val| val.clone()).collect::<Vec<u8>>(),
        result
            .iter()
            .map(|&byte| format!("%{:02x}", byte))
            .collect::<String>(),
    )
}
