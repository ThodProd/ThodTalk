use anyhow::{Result, ensure};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use ::tls_codec::{Serialize, Deserialize};
use openmls_traits::OpenMlsProvider;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

pub struct Identity {
    pub provider: OpenMlsRustCrypto,
    pub signer: SignatureKeyPair,
    credential: CredentialWithKey,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct IdentityState {
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    storage: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Identity {
    pub fn public_key(&self) -> Vec<u8> { self.signer.to_public_vec() }
    pub fn save(&self, store: &crate::storage::Store) -> Result<()> {
        // Serialize through a borrowed representation so secret keys are not cloned.
        #[derive(serde::Serialize)]
        struct State<'a> { signer: &'a SignatureKeyPair, credential: &'a CredentialWithKey, storage: Vec<(&'a Vec<u8>, &'a Vec<u8>)> }
        let values = self.provider.storage().values.read().map_err(|_| anyhow::anyhow!("Хранилище ключей заблокировано"))?;
        store.save("identity", &State { signer: &self.signer, credential: &self.credential, storage: values.iter().collect() })
    }
    pub fn load_or_create(store: &crate::storage::Store, id: &[u8]) -> Result<Self> {
        if let Some(state) = store.load::<IdentityState>("identity")? {
            let provider = OpenMlsRustCrypto::default();
            *provider.storage().values.write().map_err(|_| anyhow::anyhow!("Хранилище ключей заблокировано"))? = state.storage.into_iter().collect();
            Ok(Self { provider, signer: state.signer, credential: state.credential })
        } else {
            let identity = Self::new(id)?;
            identity.save(store)?;
            Ok(identity)
        }
    }
    pub fn new(id: &[u8]) -> Result<Self> {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm())?;
        let credential = CredentialWithKey {
            credential: BasicCredential::new(id.to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        };
        Ok(Self { provider, signer, credential })
    }
    pub fn key_package(&self) -> Result<Vec<u8>> {
        let bundle = KeyPackage::builder().build(SUITE, &self.provider, &self.signer, self.credential.clone())?;
        Ok(bundle.key_package().tls_serialize_detached()?)
    }
    pub fn create_group(&self) -> Result<MlsGroup> {
        let config = MlsGroupCreateConfig::builder().ciphersuite(SUITE).use_ratchet_tree_extension(true).build();
        Ok(MlsGroup::new(&self.provider, &self.signer, &config, self.credential.clone())?)
    }
    pub fn add(&self, group: &mut MlsGroup, key_package: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        ensure!(key_package.len() < 64 * 1024, "Слишком большой пакет ключей");
        let package = KeyPackageIn::tls_deserialize_exact(key_package)?.validate(self.provider.crypto(), ProtocolVersion::Mls10)?;
        let (commit, welcome, _) = group.add_members(&self.provider, &self.signer, &[package])?;
        group.merge_pending_commit(&self.provider)?;
        Ok((commit.tls_serialize_detached()?, welcome.tls_serialize_detached()?))
    }
    pub fn join(&self, welcome: &[u8]) -> Result<MlsGroup> {
        let MlsMessageBodyIn::Welcome(welcome) = MlsMessageIn::tls_deserialize_exact(welcome)?.extract() else { anyhow::bail!("Ожидалось приглашение MLS"); };
        Ok(StagedWelcome::new_from_welcome(&self.provider, &MlsGroupJoinConfig::default(), welcome, None)?.into_group(&self.provider)?)
    }
    pub fn encrypt(&self, group: &mut MlsGroup, plaintext: &[u8]) -> Result<Vec<u8>> {
        ensure!(plaintext.len() <= 64 * 1024, "Слишком большое сообщение");
        Ok(group.create_message(&self.provider, &self.signer, plaintext)?.tls_serialize_detached()?)
    }
    pub fn receive(&self, group: &mut MlsGroup, wire: &[u8]) -> Result<Option<Vec<u8>>> {
        ensure!(wire.len() <= 1024 * 1024, "Слишком большой пакет");
        let msg = MlsMessageIn::tls_deserialize_exact(wire)?.try_into_protocol_message()?;
        let processed = group.process_message(&self.provider, msg)?;
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => Ok(Some(message.into_bytes())),
            ProcessedMessageContent::StagedCommitMessage(commit) => { group.merge_staged_commit(&self.provider, *commit)?; Ok(None) },
            _ => anyhow::bail!("Неподдерживаемое сообщение протокола"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_group_rejects_replay_tampering_and_outsider() {
        let alice = Identity::new(b"alice").unwrap();
        let bob = Identity::new(b"bob").unwrap();
        let eve = Identity::new(b"eve").unwrap();
        let mut a = alice.create_group().unwrap();
        let (_, welcome) = alice.add(&mut a, &bob.key_package().unwrap()).unwrap();
        assert!(eve.join(&welcome).is_err());
        let mut b = bob.join(&welcome).unwrap();
        let plaintext = b"private conversation";
        let wire = alice.encrypt(&mut a, plaintext).unwrap();
        assert!(!wire.windows(plaintext.len()).any(|w| w == plaintext));
        let mut modified = wire.clone(); let last = modified.len() - 1; modified[last] ^= 1;
        assert!(bob.receive(&mut b, &modified).is_err());
        assert_eq!(bob.receive(&mut b, &wire).unwrap().unwrap(), plaintext);
        assert!(bob.receive(&mut b, &wire).is_err());
    }
    #[test]
    fn identity_and_ratchet_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::open(dir.path().to_owned()).unwrap();
        let alice = Identity::load_or_create(&store, b"alice").unwrap();
        let bob = Identity::new(b"bob").unwrap();
        let mut a = alice.create_group().unwrap();
        let (_, welcome) = alice.add(&mut a, &bob.key_package().unwrap()).unwrap();
        let mut b = bob.join(&welcome).unwrap();
        let group_id = a.group_id().clone();
        let public_key = alice.public_key();
        alice.save(&store).unwrap();
        drop(a); drop(alice);
        let restored = Identity::load_or_create(&store, b"alice").unwrap();
        assert_eq!(restored.public_key(), public_key);
        let mut a = MlsGroup::load(restored.provider.storage(), &group_id).unwrap().unwrap();
        let wire = restored.encrypt(&mut a, b"after restart").unwrap();
        assert_eq!(bob.receive(&mut b, &wire).unwrap().unwrap(), b"after restart");
    }
    #[test]
    fn removed_member_cannot_read_new_epoch() {
        let alice = Identity::new(b"alice").unwrap();
        let bob = Identity::new(b"bob").unwrap();
        let charlie = Identity::new(b"charlie").unwrap();
        let mut a = alice.create_group().unwrap();
        let (_, welcome) = alice.add(&mut a, &bob.key_package().unwrap()).unwrap();
        let mut b = bob.join(&welcome).unwrap();
        let (commit, welcome) = alice.add(&mut a, &charlie.key_package().unwrap()).unwrap();
        bob.receive(&mut b, &commit).unwrap();
        let mut c = charlie.join(&welcome).unwrap();
        let (commit, _, _) = a.remove_members(&alice.provider, &alice.signer, &[b.own_leaf_index()]).unwrap();
        a.merge_pending_commit(&alice.provider).unwrap();
        charlie.receive(&mut c, &commit.tls_serialize_detached().unwrap()).unwrap();
        let wire = alice.encrypt(&mut a, b"after removal").unwrap();
        assert!(bob.receive(&mut b, &wire).is_err());
        assert_eq!(charlie.receive(&mut c, &wire).unwrap().unwrap(), b"after removal");
    }
}
