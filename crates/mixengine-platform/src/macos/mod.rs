//! macOS implementations of the platform traits.

mod home;

/// The macOS host.
#[derive(Debug, Default)]
pub(crate) struct Host {
    home: home::Home,
}

impl Host {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl crate::Host for Host {
    fn home_dirs(&self) -> &dyn crate::HomeDirs {
        &self.home
    }
}
