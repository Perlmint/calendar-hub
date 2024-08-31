use jwt::{Store, VerifyingAlgorithm};
use rsa::{pkcs8::AssociatedOid, Pkcs1v15Sign, RsaPublicKey};
use sha2::Digest;
use std::{collections::BTreeMap, ops::Deref};

#[derive(Default)]
pub struct Keyring(BTreeMap<String, RsaVerifying>);

impl Keyring {
    pub async fn fetch(&mut self) -> anyhow::Result<()> {
        #[derive(serde::Deserialize)]
        struct Key {
            n: String,
            e: String,
            kid: String,
            alg: String,
        }
        #[derive(serde::Deserialize)]
        struct R {
            keys: Vec<Key>,
        }
        let resp: R = reqwest::get("https://www.googleapis.com/oauth2/v3/certs")
            .await?
            .json()
            .await?;

        self.0.clear();

        for key in resp.keys {
            self.0.insert(
                key.kid.to_string(),
                RsaVerifying(
                    rsa::RsaPublicKey::new(
                        rsa::BigUint::from_bytes_be(&base64_url::decode(&key.n).unwrap()),
                        rsa::BigUint::from_bytes_be(&base64_url::decode(&key.e).unwrap()),
                    )
                    .unwrap(),
                    match key.alg.as_str() {
                        "RS256" => RsAlgorithm::Rs256,
                        "RS384" => RsAlgorithm::Rs384,
                        "RS512" => RsAlgorithm::Rs512,
                        alg => unreachable!("Invalid algorithm type - {alg}"),
                    },
                ),
            );
        }

        Ok(())
    }
}

impl Deref for Keyring {
    type Target = BTreeMap<String, RsaVerifying>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Store for Keyring {
    type Algorithm = RsaVerifying;

    fn get(&self, key_id: &str) -> Option<&Self::Algorithm> {
        self.0.get(key_id)
    }
}

pub enum RsAlgorithm {
    Rs256,
    Rs384,
    Rs512,
}
pub struct RsaVerifying(RsaPublicKey, RsAlgorithm);

impl RsaVerifying {
    fn verify_with_hash<H: Digest + AssociatedOid>(
        &self,
        header: &str,
        claims: &str,
        signature: &[u8],
    ) -> Result<bool, jwt::Error> {
        match self.0.verify(
            Pkcs1v15Sign::new::<H>(),
            {
                let mut hasher = H::new();
                hasher.update(header);
                hasher.update(".");
                hasher.update(claims);
                &hasher.finalize()
            },
            signature,
        ) {
            Ok(()) => Ok(true),
            Err(e) if e == rsa::Error::Verification => Ok(false),
            Err(_) => Err(jwt::Error::InvalidSignature),
        }
    }
}

impl VerifyingAlgorithm for RsaVerifying {
    fn algorithm_type(&self) -> jwt::AlgorithmType {
        match self.1 {
            RsAlgorithm::Rs256 => jwt::AlgorithmType::Rs256,
            RsAlgorithm::Rs384 => jwt::AlgorithmType::Rs384,
            RsAlgorithm::Rs512 => jwt::AlgorithmType::Rs512,
        }
    }

    fn verify_bytes(
        &self,
        header: &str,
        claims: &str,
        signature: &[u8],
    ) -> Result<bool, jwt::Error> {
        match self.1 {
            RsAlgorithm::Rs256 => self.verify_with_hash::<sha2::Sha256>(header, claims, signature),
            RsAlgorithm::Rs384 => self.verify_with_hash::<sha2::Sha384>(header, claims, signature),
            RsAlgorithm::Rs512 => self.verify_with_hash::<sha2::Sha512>(header, claims, signature),
        }
    }
}
