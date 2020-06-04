mod fork_choice_store;

use crate::{metrics, BeaconChainTypes, BeaconSnapshot};
use lmd_ghost::{Error as LmdGhostError, QueuedAttestation};
use parking_lot::{RwLock, RwLockReadGuard};
use proto_array_fork_choice::ProtoArrayForkChoice;
use ssz::{Decode, Encode};
use ssz_derive::{Decode, Encode};
use std::sync::Arc;
use store::{DBColumn, Error as StoreError, StoreItem};
use types::{BeaconBlock, BeaconState, ChainSpec, Epoch, Hash256, IndexedAttestation, Slot};

pub use fork_choice_store::{Error as ForkChoiceStoreError, ForkChoiceStore};
pub use lmd_ghost::InvalidAttestation;

#[derive(Debug)]
pub enum Error {
    InvalidAttestation(InvalidAttestation),
    BackendError(LmdGhostError<ForkChoiceStoreError>),
    ForkChoiceStoreError(ForkChoiceStoreError),
    InvalidProtoArrayBytes(String),
    InvalidForkChoiceStoreBytes(ForkChoiceStoreError),
    UnableToReadSlot,
}

/// Wraps the `LmdGhost` fork choice and provides:
///
/// - Initialization and persistence functions.
/// - Metrics.
/// - Helper methods.
///
/// This struct does not perform any fork choice "business logic", that is all delegated to
/// `LmdGhost`.
pub struct ForkChoice<T: BeaconChainTypes> {
    backend: RwLock<
        // TODO: clean up this insane type def.
        lmd_ghost::ForkChoice<
            ForkChoiceStore<<T as BeaconChainTypes>::Store, <T as BeaconChainTypes>::EthSpec>,
            <T as BeaconChainTypes>::EthSpec,
        >,
    >,
}

impl<T: BeaconChainTypes> PartialEq for ForkChoice<T> {
    fn eq(&self, other: &Self) -> bool {
        *self.backend.read() == *other.backend.read()
    }
}

impl<T: BeaconChainTypes> ForkChoice<T> {
    /// Instantiate a new fork chooser from the genesis `BeaconSnapshot`.
    pub fn from_genesis(
        store: Arc<T::Store>,
        slot_clock: &T::SlotClock,
        genesis: &BeaconSnapshot<T::EthSpec>,
        spec: &ChainSpec,
    ) -> Result<Self, Error> {
        let fc_store = ForkChoiceStore::from_genesis(store, slot_clock, genesis, spec)
            .map_err(Error::ForkChoiceStoreError)?;

        let backend = lmd_ghost::ForkChoice::from_genesis(
            fc_store,
            genesis.beacon_block_root,
            &genesis.beacon_block.message,
            &genesis.beacon_state,
        )?;

        Ok(Self {
            backend: RwLock::new(backend),
        })
    }

    /// Run the fork choice rule to determine the head.
    pub fn find_head(&self, current_slot: Slot) -> Result<Hash256, Error> {
        let _timer = metrics::start_timer(&metrics::FORK_CHOICE_FIND_HEAD_TIMES);
        self.backend
            .write()
            .get_head(current_slot)
            .map_err(Into::into)
    }

    /// Process an attestation which references `block` in `attestation.data.beacon_block_root`.
    ///
    /// Assumes the attestation is valid.
    pub fn process_indexed_attestation(
        &self,
        current_slot: Slot,
        attestation: &IndexedAttestation<T::EthSpec>,
    ) -> Result<(), Error> {
        let _timer = metrics::start_timer(&metrics::FORK_CHOICE_PROCESS_ATTESTATION_TIMES);

        self.backend
            .write()
            .on_attestation(current_slot, attestation)?;

        Ok(())
    }

    /// Process all attestations in the given `block`.
    ///
    /// Assumes the block (and therefore its attestations) are valid. It is a logic error to
    /// provide an invalid block.
    pub fn process_block(
        &self,
        current_slot: Slot,
        state: &BeaconState<T::EthSpec>,
        block: &BeaconBlock<T::EthSpec>,
        block_root: Hash256,
    ) -> Result<(), Error> {
        let _timer = metrics::start_timer(&metrics::FORK_CHOICE_PROCESS_BLOCK_TIMES);

        self.backend
            .write()
            .on_block(current_slot, block, block_root, state)?;

        Ok(())
    }

    /// Returns true if the given block is known to fork choice.
    pub fn contains_block(&self, block_root: &Hash256) -> bool {
        self.backend.read().proto_array().contains_block(block_root)
    }

    /// Returns the state root for the given block root.
    pub fn block_slot_and_state_root(&self, block_root: &Hash256) -> Option<(Slot, Hash256)> {
        self.backend
            .read()
            .proto_array()
            .get_block(block_root)
            .map(|block| (block.slot, block.state_root))
    }

    /// Returns the latest message for a given validator, if any.
    ///
    /// Returns `(block_root, block_slot)`.
    pub fn latest_message(&self, validator_index: usize) -> Option<(Hash256, Epoch)> {
        self.backend
            .read()
            .proto_array()
            .latest_message(validator_index)
    }

    /// Trigger a prune on the underlying fork choice backend.
    pub fn prune(&self) -> Result<(), Error> {
        self.backend.write().prune().map_err(Into::into)
    }

    /// Returns a read-lock to the backend fork choice algorithm.
    ///
    /// Should only be used when encoding/decoding during troubleshooting.
    pub fn backend(
        &self,
    ) -> RwLockReadGuard<
        lmd_ghost::ForkChoice<
            ForkChoiceStore<<T as BeaconChainTypes>::Store, <T as BeaconChainTypes>::EthSpec>,
            <T as BeaconChainTypes>::EthSpec,
        >,
    > {
        self.backend.read()
    }

    /// Instantiate `Self` from some `PersistedForkChoice` generated by a earlier call to
    /// `Self::to_persisted`.
    pub fn from_persisted(
        persisted: PersistedForkChoice,
        store: Arc<T::Store>,
    ) -> Result<Self, Error> {
        let fc_store = ForkChoiceStore::from_bytes(&persisted.fc_store_bytes, store)
            .map_err(Error::InvalidForkChoiceStoreBytes)?;
        let proto_array = ProtoArrayForkChoice::from_bytes(&persisted.proto_array_bytes)
            .map_err(Error::InvalidProtoArrayBytes)?;

        Ok(Self {
            backend: RwLock::new(lmd_ghost::ForkChoice::from_components(
                fc_store,
                proto_array,
                persisted.genesis_block_root,
                persisted.queued_attestations,
            )),
        })
    }

    /// Takes a snapshot of `Self` and stores it in `PersistedForkChoice`, allowing this struct to
    /// be instantiated again later.
    pub fn to_persisted(&self) -> PersistedForkChoice {
        let backend = self.backend.read();

        PersistedForkChoice {
            fc_store_bytes: backend.fc_store().to_bytes(),
            proto_array_bytes: backend.proto_array().as_bytes(),
            queued_attestations: backend.queued_attestations().to_vec(),
            genesis_block_root: *backend.genesis_block_root(),
        }
    }
}

impl From<LmdGhostError<ForkChoiceStoreError>> for Error {
    fn from(e: LmdGhostError<ForkChoiceStoreError>) -> Error {
        match e {
            LmdGhostError::InvalidAttestation(e) => Error::InvalidAttestation(e),
            e => Error::BackendError(e),
        }
    }
}

/// Helper struct that is used to encode/decode the state of the `ForkChoice` as SSZ bytes.
///
/// This is used when persisting the state of the `BeaconChain` to disk.
#[derive(Encode, Decode, Clone)]
pub struct PersistedForkChoice {
    fc_store_bytes: Vec<u8>,
    proto_array_bytes: Vec<u8>,
    queued_attestations: Vec<QueuedAttestation>,
    genesis_block_root: Hash256,
}

impl StoreItem for PersistedForkChoice {
    fn db_column() -> DBColumn {
        DBColumn::ForkChoice
    }

    fn as_store_bytes(&self) -> Vec<u8> {
        self.as_ssz_bytes()
    }

    fn from_store_bytes(bytes: &[u8]) -> std::result::Result<Self, StoreError> {
        Self::from_ssz_bytes(bytes).map_err(Into::into)
    }
}
