//! Bounds native Array traversal across accessors, species and callback reentry.
//! A top-level operation owns the budget; catching a nested error cannot reset it.

use std::{cell::Cell, rc::Rc};

use crate::{Context, JsNativeError, JsResult};

use super::native_recursion::NativeRecursion;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WorkState {
    depth: usize,
    iterations: u64,
    bytes: u64,
}

/// Does not borrow Context while JavaScript callbacks execute.
pub(crate) struct NativeWork {
    state: Rc<Cell<WorkState>>,
    _recursion: NativeRecursion,
}

impl Context {
    pub(crate) fn enter_native_work(&self) -> JsResult<NativeWork> {
        let recursion = self.enter_native_recursion()?;
        let state = &self.vm.native_work;
        let mut next = state.get();
        if next.depth == 0 {
            next.iterations = self.runtime_limits().native_iteration_limit();
            next.bytes = self.runtime_limits().native_allocation_limit();
        }
        next.depth += 1;
        state.set(next);
        Ok(NativeWork {
            state: Rc::clone(state),
            _recursion: recursion,
        })
    }
}

impl NativeWork {
    /// Admit a known full traversal before allocating from its advertised length.
    /// Early-return algorithms charge actual iterations instead.
    pub(crate) fn check_iterations(&self, count: u64) -> JsResult<()> {
        if count > self.state.get().iterations {
            return Err(limit_error());
        }
        Ok(())
    }

    pub(crate) fn step(&self) -> JsResult<()> {
        self.iterations(1)
    }

    pub(crate) fn iterations(&self, count: u64) -> JsResult<()> {
        let mut next = self.state.get();
        next.iterations = next.iterations.checked_sub(count).ok_or_else(limit_error)?;
        self.state.set(next);
        Ok(())
    }

    pub(crate) fn bytes(&self, count: u64) -> JsResult<()> {
        let mut next = self.state.get();
        next.bytes = next.bytes.checked_sub(count).ok_or_else(limit_error)?;
        self.state.set(next);
        Ok(())
    }

    /// Charge before converting to a native size or asking the allocator.
    pub(crate) fn reserve<T>(&self, values: &mut Vec<T>, additional: u64) -> JsResult<()> {
        let required = (values.len() as u64)
            .checked_add(additional)
            .ok_or_else(limit_error)?;
        let capacity = values.capacity() as u64;
        if required <= capacity {
            return Ok(());
        }
        let element_bytes = size_of::<T>() as u64;
        let preferred = required.max(capacity.saturating_mul(2));
        let preferred_bytes = preferred
            .saturating_sub(capacity)
            .saturating_mul(element_bytes);
        let target = if preferred_bytes <= self.state.get().bytes {
            preferred
        } else {
            required
        };
        let bytes = target
            .checked_sub(capacity)
            .and_then(|n| n.checked_mul(element_bytes))
            .ok_or_else(limit_error)?;
        self.bytes(bytes)?;
        let additional =
            usize::try_from(target - values.len() as u64).map_err(|_| limit_error())?;
        values
            .try_reserve_exact(additional)
            .map_err(|_| limit_error())
    }

    pub(crate) fn append_string(
        &self,
        values: &mut Vec<u16>,
        value: &crate::JsString,
    ) -> JsResult<()> {
        self.reserve(values, value.len() as u64)?;
        values.extend(value.iter());
        Ok(())
    }
}

impl Drop for NativeWork {
    fn drop(&mut self) {
        let mut next = self.state.get();
        next.depth -= 1;
        self.state.set(next);
    }
}

fn limit_error() -> crate::JsError {
    JsNativeError::range()
        .with_message("native Array work limit exceeded")
        .into()
}

#[cfg(test)]
mod tests;
