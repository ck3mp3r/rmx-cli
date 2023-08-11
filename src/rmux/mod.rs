pub mod cli;
pub mod config;

use std::{
    env::{self, current_dir, var},
    error::Error,
    fs::{self, read_to_string},
    io::stdin,
    rc::Rc,
};

use crate::{cmd::CmdRunner, tmux::Tmux};

use self::config::{FlexDirection, Pane, Session};

const TEMPLATE: &str = include_str!("rmux.yaml");

#[derive(Debug)]
pub(crate) struct Rmux<R: CmdRunner> {
    pub config_path: String,
    cmd_runner: Rc<R>,
}

impl<R: CmdRunner> Rmux<R> {
    pub(crate) fn new(config_path: String, cmd_runner: Rc<R>) -> Self {
        Self {
            config_path: config_path.replace("~", env::var("HOME").unwrap().as_str()),
            cmd_runner,
        }
    }

    pub(crate) fn new_config(
        &self,
        name: &String,
        copy: &Option<String>,
        pwd: &bool,
    ) -> Result<(), Box<dyn Error>> {
        let mut _config_file = self.config_path.clone();
        match pwd {
            true => {
                // create local config
                _config_file = ".rmux.yaml".to_string();
            }
            false => {
                // create config dir if it doesn't exist
                self.cmd_runner
                    .run(&format!("mkdir -p {}", self.config_path))?;

                // create named config
                _config_file = format!("{}/{}.yaml", self.config_path, name);
            }
        }

        match copy {
            // copy existing configuration
            Some(copy) => {
                self.cmd_runner.run(&format!(
                    "cp {}/{}.yaml {}",
                    self.config_path, copy, _config_file
                ))?;
            }
            // create configuration from template
            None => {
                let tpl = TEMPLATE.replace("{name}", name);
                self.cmd_runner
                    .run(&format!("echo '{}' > {}", tpl, _config_file))?;
            }
        }

        // open editor with new config file
        self.cmd_runner.run(&format!(
            "{} {}",
            var("EDITOR").unwrap_or_else(|_| "vim".to_string()),
            _config_file
        ))
    }

    pub(crate) fn edit_config(&self, name: &String) -> Result<(), Box<dyn Error>> {
        self.cmd_runner.run(&format!(
            "{} {}/{}.yaml",
            var("EDITOR").unwrap_or_else(|_| "vim".to_string()),
            self.config_path,
            name
        ))
    }

    pub(crate) fn delete_config(&self, name: &String, force: &bool) -> Result<(), Box<dyn Error>> {
        if !force {
            println!("Are you sure you want to delete {}? [y/N]", name);
            let mut input = String::new();
            stdin().read_line(&mut input)?;
            if input.trim() != "y" {
                println!("Aborting.");
                return Ok(());
            }
        }
        fs::remove_file(format!("{}/{}.yaml", &self.config_path, name))?;
        Ok(())
    }

    pub(crate) fn start_session(
        &self,
        name: &Option<String>,
        attach: &bool,
    ) -> Result<(), Box<dyn Error>> {
        // figure out the config to load
        let config = match name {
            Some(name) => format!("{}/{}.yaml", &self.config_path, name),
            None => {
                let local_config = current_dir()?.join(".rmux").to_string_lossy().to_string();
                format!("{}.yaml", local_config)
            }
        };

        // Read the YAML file into a string
        let config_str = read_to_string(config)?;

        // Parse the YAML into a `Session` struct
        let session: Session = serde_yaml::from_str(&config_str)?;
        dbg!(&session);

        // create tmux client
        let tmux = Tmux::new(
            &Some(session.name),
            &session.path.to_owned(),
            Rc::clone(&self.cmd_runner),
        );

        // check if session already exists
        if tmux.session_exists() {
            println!("Session already exists");
            if *attach {
                if tmux.is_inside_session() {
                    tmux.switch_client()?;
                } else {
                    tmux.attach_session()?;
                }
            }
            return Ok(());
        }

        let dimensions = tmux.get_dimensions()?;

        // create the session
        tmux.create_session()?;

        // iterate windows
        for i in 0..session.windows.len() {
            let window = &session.windows[i];

            let idx: i32 = (i + 1).try_into().unwrap();

            let window_path =
                self.sanitize_path(&window.path, &session.path.to_owned().unwrap().clone());

            // create new window
            let window_id = tmux.new_window(&window.name, &window_path.to_string())?;

            // register commands
            tmux.register_commands(&window_id, &window.commands);

            // delete first window and move others
            if idx == 1 {
                tmux.delete_window(1)?;
                tmux.move_windows()?;
            }

            // create layout string
            let layout = self.generate_layout_string(
                &window_id,
                &window_path,
                &window.panes,
                dimensions.width,
                dimensions.height,
                &window.flex_direction,
                0,
                0,
                &tmux,
                0,
            )?;

            // apply layout to window
            tmux.select_layout(
                &window_id,
                &format!("{},{}", tmux.layout_checksum(&layout), layout),
            )?;
        }

        if *attach {
            if tmux.is_inside_session() {
                tmux.switch_client()?;
            } else {
                tmux.attach_session()?;
            }
        }

        // run all registered commands
        tmux.flush_commands()?;

        Ok(())
    }

    fn generate_layout_string(
        &self,
        window_id: &String,
        window_path: &String,
        panes: &[Pane],
        width: usize,
        height: usize,
        direction: &Option<FlexDirection>,
        start_x: usize,
        start_y: usize,
        tmux: &Tmux<R>,
        depth: usize,
    ) -> Result<String, Box<dyn Error>> {
        let total_flex = panes.iter().map(|p| p.flex.unwrap_or(1)).sum::<usize>();
        dbg!(total_flex, width, height, start_x, start_y);

        let mut current_x = start_x;
        let mut current_y = start_y;
        let mut pane_strings: Vec<String> = Vec::new();

        let mut dividers = 0;

        for (index, pane) in panes.iter().enumerate() {
            let flex = pane.flex.unwrap_or(1);

            let (pane_width, pane_height, next_x, next_y) = match direction {
                Some(FlexDirection::Column) => {
                    let w = if index == panes.len() - 1 {
                        if current_x > width {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Width underflow detected",
                            )));
                        }
                        width - current_x // give the remaining width to the last pane
                    } else if depth > 0 || index > 0 {
                        width * flex / total_flex - dividers
                    } else {
                        width * flex / total_flex
                    };
                    (w, height, current_x + w + 1, current_y)
                }
                _ => {
                    let h = if index == panes.len() - 1 {
                        if current_y > height {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Height underflow detected",
                            )));
                        }
                        height - current_y // give the remaining height to the last pane
                    } else if depth > 0 || index > 0 {
                        height * flex / total_flex - dividers
                    } else {
                        height * flex / total_flex
                    };
                    (width, h, current_x, current_y + h + 1)
                }
            };

            // Increment divider count after calculating position and dimension for this pane
            if depth > 0 || index > 0 {
                dividers += 1;
            }

            let path = self.sanitize_path(&pane.path, &window_path);

            // Create panes in tmux as we go
            let pane_id = if index > 0 {
                tmux.split_window(window_id, &path)?
            } else {
                tmux.get_current_pane(window_id)?
            };
            tmux.select_layout(window_id, &"tiled".to_string())?;

            dbg!(&pane_id);

            if let Some(sub_panes) = &pane.panes {
                pane_strings.push(self.generate_layout_string(
                    window_id,
                    window_path,
                    sub_panes,
                    pane_width,
                    pane_height,
                    &pane.flex_direction,
                    current_x,
                    current_y,
                    &tmux,
                    depth + 1,
                )?);
            } else {
                pane_strings.push(format!(
                    "{0}x{1},{2},{3},{4}",
                    pane_width,
                    pane_height,
                    current_x,
                    current_y,
                    pane_id.replace("%", "")
                ));
            }

            current_x = next_x;
            current_y = next_y;
            dbg!(next_x, next_y);
            tmux.register_commands(&pane_id, &pane.commands);
        }

        if pane_strings.len() > 1 {
            match direction {
                Some(FlexDirection::Column) => Ok(format!(
                    "{}x{},0,0{{{}}}",
                    width,
                    height,
                    pane_strings.join(",")
                )),
                _ => Ok(format!(
                    "{}x{},0,0[{}]",
                    width,
                    height,
                    pane_strings.join(",")
                )),
            }
        } else {
            Ok(format!("{}x{},0,0", width, height))
        }
    }

    pub(crate) fn stop_session(&self, name: &Option<String>) -> Result<(), Box<dyn Error>> {
        let tmux = Tmux::new(&name, &None, Rc::clone(&self.cmd_runner));
        tmux.stop_session(&name)
    }

    pub(crate) fn list_config(&self) -> Result<(), Box<dyn Error>> {
        let mut entries: Vec<String> = Vec::new();

        for entry in fs::read_dir(&self.config_path)? {
            let path = entry?.path();
            if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
                if ext == "yaml" {
                    if let Some(file_name) = path.file_stem().and_then(|name| name.to_str()) {
                        entries.push(file_name.to_string());
                    }
                }
            }
        }

        if entries.is_empty() {
            println!("No configurations found.");
        } else {
            println!("Available configurations:");
            println!("{}", entries.join("\n"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn cmd_runner(&self) -> &R {
        &self.cmd_runner
    }

    fn sanitize_path(&self, path: &Option<String>, window_path: &String) -> String {
        match &path {
            Some(path) => {
                if path.starts_with("/") || path.starts_with("~") {
                    path.to_string()
                } else if path == "." {
                    window_path.to_string()
                } else {
                    format!("{}/{}", window_path, path)
                }
            }
            None => window_path.to_string(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::Rmux;
    use crate::cmd::test::MockCmdRunner;
    use crate::rmux::TEMPLATE;
    use std::{
        env::{current_dir, var},
        rc::Rc,
    };

    #[test]
    fn new_config_copy() {
        let session_name = "test";
        let cmd_runner = Rc::new(MockCmdRunner::new());
        let rmux = Rmux::new("/tmp/rmux".to_string(), Rc::clone(&cmd_runner));

        rmux.new_config(
            &session_name.to_string(),
            &Some(String::from("bla")),
            &false,
        )
        .unwrap();
        let editor = var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let cmds = rmux.cmd_runner().cmds().borrow();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], format!("mkdir -p {}", rmux.config_path));
        assert_eq!(
            cmds[1],
            format!(
                "cp {}/{}.yaml {}/{}.yaml",
                rmux.config_path, "bla", rmux.config_path, session_name
            )
        );
        assert_eq!(cmds[2], format!("{} /tmp/rmux/test.yaml", editor));
    }
    #[test]
    fn new_config_local() {
        let session_name = "test";
        let cmd_runner = Rc::new(MockCmdRunner::new());
        let rmux = Rmux::new(".".to_string(), Rc::clone(&cmd_runner));

        rmux.new_config(&session_name.to_string(), &None, &true)
            .unwrap();
        let editor = var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let cmds = rmux.cmd_runner().cmds().borrow();
        let tpl = TEMPLATE.replace("{name}", &session_name.to_string());
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], format!("echo '{}' > .rmux.yaml", tpl));
        assert_eq!(cmds[1], format!("{} .rmux.yaml", editor));
    }

    #[test]
    fn edit_config() {
        let session_name = "test";
        let cmd_runner = Rc::new(MockCmdRunner::new());
        let rmux = Rmux::new("/tmp/rmux".to_string(), Rc::clone(&cmd_runner));

        rmux.edit_config(&session_name.to_string()).unwrap();
        let editor = var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let cmds = rmux.cmd_runner().cmds().borrow();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0], format!("{} /tmp/rmux/test.yaml", editor));
    }

    #[test]
    fn stop_session() {
        let cwd = current_dir().unwrap();

        let session_name = "test";
        let cmd_runner = Rc::new(MockCmdRunner::new());
        let rmux = Rmux::new(
            format!("{}/src/rmux/test", cwd.to_string_lossy()),
            Rc::clone(&cmd_runner),
        );

        let res = rmux.stop_session(&Some(session_name.to_string()));
        let cmds = rmux.cmd_runner().cmds().borrow();
        match res {
            Ok(_) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "tmux display-message -p \"#{session_base_path}\"");
                assert_eq!(cmds[1], "tmux kill-session -t test")
            }
            Err(e) => assert_eq!(e.to_string(), "Session not found"),
        }
    }

    #[test]
    fn start_session() {
        let cwd = current_dir().unwrap();

        let session_name = "test";
        let cmd_runner = Rc::new(MockCmdRunner::new());
        let rmux = Rmux::new(
            format!("{}/src/rmux/test", cwd.to_string_lossy()),
            Rc::clone(&cmd_runner),
        );

        let res = rmux.start_session(&Some(session_name.to_string()), &true);
        let mut cmds = rmux.cmd_runner().cmds().borrow().clone();
        match res {
            Ok(_) => {
                // assert_eq!(cmds.len(), 1);
                assert_eq!(cmds.remove(0).to_string(), "tmux has-session -t test");
                assert_eq!(cmds.remove(0).to_string(), "printenv TMUX");
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux display-message -p \"width: #{window_width}\nheight: #{window_height}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux new-session -d -s test -c /tmp"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux new-window -Pd -t test -n code -c /tmp -F \"#{window_id}\""
                );
                assert_eq!(cmds.remove(0).to_string(), "tmux kill-window -t test:1");
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux move-window -r -s test -t test"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux display-message -t test:@1 -p \"#P\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@1 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux display-message -t test:@1 -p \"#P\""
                );
                // // assert_eq!(cmds.remove(0).to_string(), "tmux kill-pane -t test:1.1");
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@1 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux split-window -t test:@1 -c /tmp -P -F \"#{pane_id}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@1 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux split-window -t test:@1 -c /tmp/src -P -F \"#{pane_id}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@1 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@1 \"9b85,160x90,0,0{80x90,0,0[80x30,0,0,2,80x59,0,31,3],79x90,81,0,4}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux new-window -Pd -t test -n infrastructure -c /tmp -F \"#{window_id}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux display-message -t test:@2 -p \"#P\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@2 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux split-window -t test:@2 -c /tmp/two -P -F \"#{pane_id}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@2 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux split-window -t test:@2 -c /tmp/three -P -F \"#{pane_id}\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@2 \"tiled\""
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux select-layout -t test:@2 \"c301,160x90,0,0{40x90,0,0,5,80x90,41,0,6,38x90,122,0,7}\""
                );
                assert_eq!(cmds.remove(0).to_string(), "printenv TMUX");
                assert_eq!(cmds.remove(0).to_string(), "tmux switch-client -t test:1");
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:@1 'echo \"hello world\"' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%1 'echo \"hello\"' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%4 'echo \"hello again\"' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%5 'echo \"hello again 1\"' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%6 'echo \"hello again 2\"' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%7 'clear' C-m"
                );
                assert_eq!(
                    cmds.remove(0).to_string(),
                    "tmux send-keys -t test:%7 'echo \"hello again 3\"' C-m"
                );
            }
            Err(e) => assert_eq!(e.to_string(), "Session not found"),
        }
    }
}
