#[cfg(feature = "tracing")]
#[allow(unused_imports)]
use tracing;

#[macro_export]
macro_rules! info {
    ($($arg:expr),*) => {
        #[cfg(feature = "tracing")]
        tracing::info!($($arg),*)
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:expr),*) => {
        #[cfg(feature = "tracing")]
        tracing::warn!($($arg),*)
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:expr),*) => {
        #[cfg(feature = "tracing")]
        tracing::error!($($arg),*)
    };
}

#[macro_export]
macro_rules! trace {
    ($($arg:expr),*) => {
        #[cfg(feature = "tracing")]
        tracing::trace!($($arg),*)
    };
}

pub use info;
