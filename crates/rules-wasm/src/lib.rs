//! The WebAssembly interface of `ankiquest-rules`, for pages that draw what the rules decide: the
//! desktop add-on's views in Anki's Qt WebEngine first (`docs/shared-rules.md`).
//!
//! The module imports nothing and needs no glue code: a page instantiates it with an empty import
//! object and calls the exports with plain numbers. `abi_version` names the exports and their
//! meaning; it changes whenever one is removed or changes meaning, so a page can refuse a module
//! it was not written for. Rules that take or return text will pass UTF-8 through `alloc` and
//! `dealloc`, as `crates/i18n-wasm` does.

/// The version of this interface. 1: `abi_version`, `heatmap_intensity`.
const ABI_VERSION: u32 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn abi_version() -> u32 {
    ABI_VERSION
}

/// [`ankiquest_rules::heatmap_intensity`]: the colour level, 0 through 4, of a day with `count`
/// reviews or cards due against the reviews per study day.
#[unsafe(no_mangle)]
pub extern "C" fn heatmap_intensity(count: u32, daily_average: f64) -> u32 {
    u32::from(ankiquest_rules::heatmap_intensity(count, daily_average))
}
