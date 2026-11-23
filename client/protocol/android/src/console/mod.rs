//! JNI bridge and `Slab`-managed handle registry for Android's `NightjarConsoleJni`
//! (`ConsoleRepository`, `HiddenStore`, `RecentsStore`, `FavouritesStore`, `LibraryCacheStore`,
//! `HomeModel`, `GameDetailModel`, and pure `Native*` JSON/policy helpers).

pub mod hidden_jni;
pub mod jni;

#[cfg(test)]
mod tests;
