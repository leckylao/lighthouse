use crate::fork_choice::compute_slots_since_epoch_start;
use types::{BeaconState, Checkpoint, EthSpec, Hash256, Slot};

/// Approximates the `Store` in "Ethereum 2.0 Phase 0 -- Beacon Chain Fork Choice":
///
/// https://github.com/ethereum/eth2.0-specs/blob/v0.12.0/specs/phase0/fork-choice.md#store
///
/// ## Detail
///
/// This is only an approximation for two reasons:
///
/// - This crate stores the actual block DAG in `ProtoArrayForkChoice`.
/// - `time` is represented using `Slot` instead of UNIX epoch `u64`.
pub trait ForkChoiceStore<T: EthSpec>: Sized {
    type Error;

    /// Instructs the implementer to ensure that it updates the current time.
    ///
    /// The implementer should call `Self::on_tick` for each slot (if any) between the last known
    /// slot and the current slot.
    fn update_time(&mut self) -> Result<(), Self::Error>;

    /// Called whenever the current time increases.
    ///
    /// ## Notes
    ///
    /// This function should only ever be passed a `time` that is equal to or greater than the
    /// previously passed value. I.e., it must be called each time the slot changes.
    ///
    /// ## Specification
    ///
    /// Implementation must be equivalent to:
    ///
    /// https://github.com/ethereum/eth2.0-specs/blob/v0.12.0/specs/phase0/fork-choice.md#on_tick
    fn on_tick(&mut self, time: Slot) -> Result<(), Self::Error> {
        let store = self;

        let previous_slot = store.get_current_slot();

        // Update store time.
        store.set_current_slot(time);

        let current_slot = store.get_current_slot();
        if !(current_slot > previous_slot
            && compute_slots_since_epoch_start::<T>(current_slot) == 0)
        {
            return Ok(());
        }

        if store.best_justified_checkpoint().epoch > store.justified_checkpoint().epoch {
            store.set_justified_checkpoint_to_best_justified_checkpoint()?;
        }

        Ok(())
    }

    /// Returns the last value passed to `Self::update_time`.
    fn get_current_slot(&self) -> Slot;

    /// Set the value to be returned by `Self::get_current_slot`.
    ///
    /// ## Notes
    ///
    /// This should only ever be called from within the `Self::on_tick` implementation.
    ///
    /// *This method only exists as a public trait function to allow for a default `Self::on_tick`
    /// implementation.*
    fn set_current_slot(&mut self, slot: Slot);

    /// Updates the `justified_checkpoint` to the `best_justified_checkpoint`.
    ///
    /// ## Notes
    ///
    /// This should only ever be called from within the `Self::on_tick` implementation.
    ///
    /// *This method only exists as a public trait function to allow for a default `Self::on_tick`
    /// implementation.*
    ///
    /// ## Specification
    ///
    /// Implementation must be equivalent to:
    ///
    /// ```ignore
    /// store.justified_checkpoint = store.best_justified_checkpoint
    /// ```
    fn set_justified_checkpoint_to_best_justified_checkpoint(&mut self) -> Result<(), Self::Error>;

    /// Returns the `justified_checkpoint`.
    fn justified_checkpoint(&self) -> &Checkpoint;

    /// Returns balances from the `state` identified by `justified_checkpoint.root`.
    fn justified_balances(&self) -> &[u64];

    /// Returns the `best_justified_checkpoint`.
    fn best_justified_checkpoint(&self) -> &Checkpoint;

    /// Returns the `finalized_checkpoint`.
    fn finalized_checkpoint(&self) -> &Checkpoint;

    /// Sets `finalized_checkpoint`.
    fn set_finalized_checkpoint(&mut self, c: Checkpoint);

    /// Sets the `justified_checkpoint`.
    fn set_justified_checkpoint(&mut self, state: &BeaconState<T>);

    /// Sets the `best_justified_checkpoint`.
    fn set_best_justified_checkpoint(&mut self, state: &BeaconState<T>);

    /// Returns the block root of an ancestor of `block_root` at the given `slot`. (Note: `slot`
    /// refers to the block is *returned*, not the one that is supplied.)
    ///
    /// The root of `state` must match the `block.state_root` of the block identified by
    /// `block_root`.
    ///
    /// ## Specification
    ///
    /// Implementation must be equivalent to:
    ///
    /// https://github.com/ethereum/eth2.0-specs/blob/v0.12.0/specs/phase0/fork-choice.md#get_ancestor
    fn get_ancestor(
        &self,
        state: &BeaconState<T>,
        block_root: Hash256,
        slot: Slot,
    ) -> Result<Hash256, Self::Error>;
}
