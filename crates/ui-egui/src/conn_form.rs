//! The connection form's fields as typed ([`Form`]), and turning them into a
//! [`ConnectionConfig`] (port numbers, validation). Drawing is in [`crate::connections`].

use std::path::PathBuf;

use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, SshAuth, SshConfig, SslMode};

/// The SSH login choices of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    Key,
    Agent,
    Password,
}

/// The connection form's fields, as typed.
#[derive(Clone, PartialEq)]
pub(crate) struct Form {
    /// The saved connection this is, or empty for a new one.
    pub id: String,
    pub name: String,
    /// Not editable in the form yet; kept so saving does not change it.
    pub kind: DbKind,
    pub file: Option<PathBuf>,
    pub color: ConnectionColor,
    pub host: String,
    pub port: String,
    pub user: String,
    /// Typed by the user; never loaded from the keyring.
    pub password: String,
    pub show_password: bool,
    pub database: String,
    pub ssl_mode: SslMode,
    pub ca_file: Option<PathBuf>,
    pub read_only: bool,
    pub save_secrets: bool,
    pub ssh_on: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_user: String,
    pub ssh_auth: Auth,
    /// The key file chosen with "Change…"; `None` picks automatically.
    pub ssh_key: Option<PathBuf>,
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &if self.password.is_empty() { "" } else { "<redacted>" })
            .field("database", &self.database)
            .field("ssh_on", &self.ssh_on)
            .finish_non_exhaustive()
    }
}

/// A port as typed: a number from 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

impl Form {
    /// An empty form for a new connection.
    pub(crate) fn new(save_secrets: bool) -> Form {
        Form::from_config(&ConnectionConfig { save_secrets, ..ConnectionConfig::default() })
    }

    /// The form showing `c` (no password: stored ones stay in the keyring).
    pub(crate) fn from_config(c: &ConnectionConfig) -> Form {
        let ssh = c.ssh.clone().unwrap_or_default();
        let (ssh_auth, ssh_key) = match &ssh.auth {
            SshAuth::Key { path } => (Auth::Key, path.clone()),
            SshAuth::Agent => (Auth::Agent, None),
            SshAuth::Password => (Auth::Password, None),
        };
        Form {
            id: c.id.clone(),
            name: c.name.clone(),
            kind: c.kind,
            file: c.file.clone(),
            color: c.color,
            host: c.host.clone(),
            port: c.port.to_string(),
            user: c.user.clone(),
            password: String::new(),
            show_password: false,
            database: c.database.clone(),
            ssl_mode: c.ssl_mode,
            ca_file: c.ca_file.clone(),
            read_only: c.read_only,
            save_secrets: c.save_secrets,
            ssh_on: c.ssh.is_some(),
            ssh_host: ssh.host,
            ssh_port: ssh.port.to_string(),
            ssh_user: ssh.user,
            ssh_auth,
            ssh_key,
        }
    }

    /// The SSH login as a config stores it.
    pub(crate) fn ssh_auth(&self) -> SshAuth {
        match self.ssh_auth {
            Auth::Key => SshAuth::Key { path: self.ssh_key.clone() },
            Auth::Agent => SshAuth::Agent,
            Auth::Password => SshAuth::Password,
        }
    }

    /// The connection the form describes, or why it is not usable yet.
    pub(crate) fn to_config(&self) -> Result<ConnectionConfig, String> {
        let port = match parse_port(&self.port) {
            Some(p) => p,
            None if self.kind.uses_file() => 0,
            None => return Err("The port must be a number from 1 to 65535.".into()),
        };
        let ssh = if self.ssh_on {
            let port = parse_port(&self.ssh_port).ok_or("The SSH port must be a number from 1 to 65535.")?;
            Some(SshConfig { host: self.ssh_host.trim().into(), port, user: self.ssh_user.trim().into(), auth: self.ssh_auth() })
        } else {
            None
        };
        let config = ConnectionConfig {
            id: self.id.clone(),
            name: self.name.trim().into(),
            kind: self.kind,
            file: self.file.clone(),
            color: self.color,
            host: self.host.trim().into(),
            port,
            user: self.user.trim().into(),
            database: self.database.trim().into(),
            ssl_mode: self.ssl_mode,
            ca_file: self.ca_file.clone(),
            ssh,
            read_only: self.read_only,
            save_secrets: self.save_secrets,
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    /// What the title line says.
    pub(crate) fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.id.is_empty() {
            "New connection".into()
        } else {
            format!("{}@{}/{}", self.user, self.host, self.database)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_keeps_the_type_and_the_file() {
        let mysql = ConnectionConfig { id: "a".into(), kind: DbKind::MySql, port: 3306, ..ConnectionConfig::default() };
        let back = Form::from_config(&mysql).to_config().unwrap();
        assert_eq!((back.kind, back.file), (DbKind::MySql, None));

        let file = Some(PathBuf::from("/x/shop.sqlite"));
        let sqlite = ConnectionConfig {
            id: "b".into(),
            kind: DbKind::Sqlite,
            file: file.clone(),
            host: String::new(),
            port: 0,
            ..ConnectionConfig::default()
        };
        let back = Form::from_config(&sqlite).to_config().unwrap();
        assert_eq!((back.kind, back.file), (DbKind::Sqlite, file));
    }
}
