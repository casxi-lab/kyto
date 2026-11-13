//! The device roles of a bench and the quantities that confirm its supply.

use serde::{Deserialize, Serialize};

use crate::document::named_enum;

named_enum! {
    /// A device role of a bench, named as instance configurations name it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Role {
        /// Switches the board's supply.
        Power = "power",
        /// Confirms the supply state with a reading of its own.
        PowerConfirmation = "power_confirmation",
        /// The board's serial console.
        Console = "console",
        /// Publishes payloads where the board reads them over TFTP.
        Delivery = "delivery",
    }
}

/// A quantity of the supply that a meter reads and that confirms the supply
/// state, in micro-units: microvolts or microamperes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    SupplyVoltage,
    SupplyCurrent,
}
