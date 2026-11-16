//! Artwork from SteamGridDB and game facts (genres, themes, time to beat) from IGDB, with the
//! player's own keys, for a game of any host. Both apps run the same [`job::Job`]: it says which
//! request to send, how long to wait and what was found, and the app only carries the requests
//! over HTTPS and keeps the [`cache::Record`] on disk. Desktop calls it directly, Android through
//! [`json::call`].
//!
//! Without keys nothing is asked: [`job::Keys::any`] is false and the apps start no job.
//!
//! The services' documentation it follows: SteamGridDB API v2 (`https://www.steamgriddb.com/api/v2`)
//! and the IGDB API (`https://api-docs.igdb.com`), which authenticates with a Twitch client ID and
//! secret through the client-credentials token flow.

pub mod cache;
pub mod job;
pub mod json;
pub mod library;
pub mod names;
pub mod pace;
pub mod redact;
pub mod services;

#[cfg(test)]
mod tests;
