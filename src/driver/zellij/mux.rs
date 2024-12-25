use std::{fs::File, io::Write, rc::Rc};

use anyhow::Result;

use crate::common::{
    cmd::{Runner, ShellRunner},
    config::Session,
    mux::Multiplexer,
};

use super::client::ZellijClient;

pub(crate) struct Zellij<R: Runner = ShellRunner> {
    client: ZellijClient<R>,
}

impl Zellij {
    pub fn new() -> Self {
        Self::new_with_runner(ShellRunner::new())
    }
}

impl<R: Runner> Zellij<R> {
    pub fn new_with_runner(runner: R) -> Self {
        Self {
            client: ZellijClient::new(Rc::new(runner)),
        }
    }

    fn session_to_layout(&self, session: &Session) -> Result<String> {
        let layout_location = format!("/tmp/{}.kdl", &session.name);
        let session_kld = session.as_kdl()?.to_string();

        let mut file = File::create(&layout_location)?;
        let sanitized_kdl = session_kld.to_string().replace("\\\"", "");
        file.write_all(sanitized_kdl.as_bytes())?;

        Ok(layout_location)
    }
}

impl<R: Runner> Multiplexer for Zellij<R> {
    fn start(
        &self,
        session: &Session,
        _config: &str,
        skip_attach: bool,
        _skip_cmds: bool,
    ) -> Result<()> {
        if self.switch(&session.name, skip_attach)? {
            return Ok(());
        }

        let layout: String = self.session_to_layout(session)?;
        let _res: () = self
            .client
            .create_session_with_layout(&session.name, layout.as_str())?;
        Ok(())
    }

    fn stop(&self, _name: &Option<String>, _skip_cmds: bool, _stop_all: bool) -> Result<()> {
        todo!()
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        todo!()
    }

    fn switch(&self, name: &str, skip_attach: bool) -> Result<bool> {
        if self.client.session_exists(name) {
            log::warn!("Session '{}' already exists", name);
            if !skip_attach {
                self.client.attach(name)?;
            }
            return Ok(true);
        }

        Ok(false)
    }

    fn get_session(&self) -> Result<Session> {
        todo!()
    }
}