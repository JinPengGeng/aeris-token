pub use aether_data_contracts::repository::emergency_chain::*;

#[cfg(feature = "postgres")]
pub use aether_data_postgres::PostgresEmergencyChainGrantRepository;
