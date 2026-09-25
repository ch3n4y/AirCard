//! The escape, wired to a phone on the end of the cable.
//!
//! Every method here opens a connection, uses it and closes it, which is why
//! [`Device`] is a name rather than a session. Two connections to one phone at
//! once do not work: two AFC sessions abort the process outright, and holding an
//! AFC session while AirTraffic talks has never been tried. The phases in
//! [`super::escape`] take turns by construction -- each opens its own session
//! through [`MediaSource`] and drops it before the next one starts -- so the
//! order they run in is the thing that keeps this safe, not a lock.

use aircard_apple_ffi::{AfcSession, AirTraffic, ZipConduit};

use super::escape::{
    Airlift, AirliftError, ArchiveUpload, AssetMover, Kind, Media, MediaSource, Result,
};

/// One iPhone, named by the udid the framework reports for it.
#[derive(Debug, Clone)]
pub struct Device {
    udid: String,
}

impl Device {
    pub fn new(udid: impl Into<String>) -> Self {
        Self { udid: udid.into() }
    }

    pub fn udid(&self) -> &str {
        &self.udid
    }

    /// The escape for this phone.
    ///
    /// A fresh value per call: nothing here is stateful, because what has to
    /// happen in order is the order of the phases, not the lifetime of a
    /// connection.
    pub fn airlift(&self) -> Airlift<Self, Self, Self> {
        Airlift::new(self.clone(), self.clone(), self.clone())
    }
}

impl MediaSource for Device {
    fn open(&self) -> Result<Box<dyn Media + '_>> {
        Ok(Box::new(Connection {
            session: AfcSession::open(&self.udid)?,
        }))
    }
}

/// An AFC session, wearing the shape the escape expects.
///
/// A newtype rather than an `impl Media for AfcSession` so that the two stay
/// separate things: `Media` is a card's point of view, and AFC is a service.
struct Connection {
    session: AfcSession,
}

impl Media for Connection {
    fn exists(&mut self, path: &str) -> bool {
        self.session.exists(path)
    }

    fn kind(&mut self, path: &str) -> Option<Kind> {
        self.session
            .stat(path)
            .map(|(_, spelling)| Kind::from_afc(&spelling))
    }

    fn read(&mut self, path: &str, limit: u64) -> Result<Vec<u8>> {
        Ok(self.session.read(path, limit)?)
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
        Ok(self.session.write(path, data)?)
    }

    fn create_directory(&mut self, path: &str) -> Result<()> {
        Ok(self.session.create_directory(path)?)
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        Ok(self.session.remove(path)?)
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>> {
        Ok(self.session.list(path)?)
    }
}

impl AssetMover for Device {
    fn move_assets(&self, assets: &[(String, String)]) -> Result<()> {
        // Failures here are reported as a move rather than as a device error so
        // that they stay worth retrying: the handshake not coming up is the one
        // failure a second attempt has a real chance with.
        let connection = AirTraffic::open(&self.udid).map_err(|error| AirliftError::Move {
            detail: error.to_string(),
        })?;
        // The connection reports per-asset progress, and a card is three files.
        // The window reports progress per card, which is the unit a person
        // waiting on a flash actually counts, so nothing reads this.
        connection
            .move_assets(assets, &mut |_, _, _| {})
            .map_err(|error| AirliftError::Move {
                detail: error.to_string(),
            })
    }
}

impl ArchiveUpload for Device {
    fn upload(&self, media_subdir: &str, archive: &[u8]) -> Result<()> {
        let conduit = ZipConduit::open(&self.udid).map_err(|error| AirliftError::Upload {
            detail: error.to_string(),
        })?;
        conduit
            .upload(media_subdir, archive)
            .map_err(|error| AirliftError::Upload {
                detail: error.to_string(),
            })
    }
}
