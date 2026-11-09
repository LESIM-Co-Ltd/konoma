//! What a host decides about a plugin's declared actions and about the typing it takes for a
//! canvas, written once for every host (konoma and the standalone app).
//!
//! Nothing here knows a terminal screen, an `App` or a message catalog: the
//! functions take the data they judge and return what the host should do (and which notice to
//! show, as an enum the host words in its own language). So a plugin's key, footer hint, help row
//! and `Space` menu item mean the same in both hosts, and the same tests hold for both.
//!
//! - [`acts`]: which declared actions belong to a screen, which one a key runs, and how they are
//!   offered (footer, help, menu) and gated.
//! - [`typing`]: the state machine of the text input a canvas can ask for -- how a key is made,
//!   when `Esc` leaves, when the typing is taken back, and the bounded line of keys and actions
//!   that wait for a session a job holds.
//! - `project` (`project-host`): discovery, serial session ownership, screen lifecycle and ordered
//!   reply/effect adoption, using context paths, policy and translated messages from the host.

pub mod acts;
pub mod bind;
pub mod launch;
#[cfg(feature = "project-host")]
pub mod project;
pub mod typing;
