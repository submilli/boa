/// Represents the limits of different runtime operations.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    /// Max stack size before an error is thrown.
    stack_size: usize,

    /// Max loop iterations before an error is thrown.
    loop_iteration: u64,

    /// Max backtrace count in exception.
    backtrace_limit: usize,

    /// Max function recursion limit
    recursion: usize,

    /// Iterations across one native Array operation, including callback reentry.
    native_iteration: u64,

    /// Bytes of native Array temporary storage across that operation.
    native_allocation: u64,
}

impl Default for RuntimeLimits {
    #[inline]
    fn default() -> Self {
        Self {
            loop_iteration: u64::MAX,
            recursion: 512,
            native_iteration: 1_000_000,
            native_allocation: 64 * 1024 * 1024,
            backtrace_limit: 50,
            stack_size: 1024 * 10,
        }
    }
}

impl RuntimeLimits {
    /// Return the loop iteration limit.
    ///
    /// If the limit is exceeded in a loop it will throw an error.
    ///
    /// The limit value [`u64::MAX`] means that there is no limit.
    #[inline]
    #[must_use]
    pub const fn loop_iteration_limit(&self) -> u64 {
        self.loop_iteration
    }

    /// Set the loop iteration limit.
    ///
    /// If the limit is exceeded in a loop it will throw an error.
    ///
    /// Setting the limit to [`u64::MAX`] means that there is no limit.
    #[inline]
    pub fn set_loop_iteration_limit(&mut self, value: u64) {
        self.loop_iteration = value;
    }

    /// Disable loop iteration limit.
    #[inline]
    pub fn disable_loop_iteration_limit(&mut self) {
        self.loop_iteration = u64::MAX;
    }

    /// Get max backtrace limit for an exception.
    ///
    /// Default is 50.
    #[inline]
    #[must_use]
    pub const fn backtrace_limit(&self) -> usize {
        self.backtrace_limit
    }

    /// Set max backtrace limit for an exception.
    #[inline]
    pub fn set_backtrace_limit(&mut self, value: usize) {
        self.backtrace_limit = value;
    }

    /// Get max stack size.
    #[inline]
    #[must_use]
    pub const fn stack_size_limit(&self) -> usize {
        self.stack_size
    }

    /// Set max stack size before an error is thrown.
    #[inline]
    pub fn set_stack_size_limit(&mut self, value: usize) {
        self.stack_size = value;
    }

    /// Get recursion limit.
    #[inline]
    #[must_use]
    pub const fn recursion_limit(&self) -> usize {
        self.recursion
    }

    /// Set recursion limit before an error is thrown.
    #[inline]
    pub fn set_recursion_limit(&mut self, value: usize) {
        self.recursion = value;
    }
}

impl RuntimeLimits {
    /// Native Array traversal limit. Default: 1,000,000 index visits and sort work
    /// units per outer operation, shared through nested accessors and callbacks.
    #[must_use]
    pub const fn native_iteration_limit(&self) -> u64 {
        self.native_iteration
    }

    /// Set the native traversal budget. Zero permits operations with no visits.
    pub fn set_native_iteration_limit(&mut self, value: u64) {
        self.native_iteration = value;
    }

    /// Native Array temporary allocation limit in bytes. Default: 64 `MiB`.
    /// Includes concatenated string code units and sort buffers; logical sparse
    /// array lengths do not allocate their advertised size.
    #[must_use]
    pub const fn native_allocation_limit(&self) -> u64 {
        self.native_allocation
    }

    /// Set the byte budget for one outer native Array operation. Values are
    /// checked before native-size conversion and fallible allocation.
    pub fn set_native_allocation_limit(&mut self, value: u64) {
        self.native_allocation = value;
    }
}
