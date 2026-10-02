//! Ввод пользователя: опции слэш-команд и поля форм.

use serenity::all::*;

/// Типизированный доступ к опциям слэш-команды.
pub struct Options<'a>(Vec<ResolvedOption<'a>>);

impl<'a> Options<'a> {
    pub fn of(command: &'a CommandInteraction) -> Self {
        Self(command.data.options())
    }

    fn value(&self, name: &str) -> Option<&ResolvedValue<'a>> {
        self.0
            .iter()
            .find(|option| option.name == name)
            .map(|option| &option.value)
    }

    pub fn str(&self, name: &str) -> Option<&'a str> {
        match self.value(name)? {
            ResolvedValue::String(value) => Some(*value),
            _ => None,
        }
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.value(name)? {
            ResolvedValue::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    pub fn channel(&self, name: &str) -> Option<ChannelId> {
        match self.value(name)? {
            ResolvedValue::Channel(channel) => Some(channel.id),
            _ => None,
        }
    }
}

/// Значение текстового поля формы; пустые и состоящие из пробелов поля считаются незаполненными.
pub fn modal_value<'a>(modal: &'a ModalInteraction, input_id: &str) -> Option<&'a str> {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            ActionRowComponent::InputText(input) if input.custom_id == input_id => {
                input.value.as_deref()
            }
            _ => None,
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
