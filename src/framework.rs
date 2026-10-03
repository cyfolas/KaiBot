//! Инфраструктура взаимодействий Discord: контракт команды, маршрутизация, права, ввод и ответы.
//!
//! Модуль ничего не знает о конкретных командах: они реализуют [`SlashCommand`] и передаются
//! в [`Registry`] при запуске (см. `commands::registry`).

mod caller;
mod command;
mod custom_id;
mod handler;
mod input;
mod reply;
mod view;

pub use caller::Caller;
pub use command::{Cx, Registry, SlashCommand};
pub use custom_id::CustomId;
pub use handler::Handler;
pub use input::{Options, modal_value, selected_roles};
pub use reply::{ephemeral, ephemeral_embed};
pub use view::{Page, confirm_row, emoji, panel, role_select};
