use futures::channel::oneshot;

use crate::FileSharingError;
use std::str::FromStr;

pub struct CommandRequest {
    pub command_request_id: u64,
    pub command: Command,
    pub response_sender: oneshot::Sender<Result<CommandResponse, FileSharingError>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Register(Register),
    ListPeers,
    ListFiles(ListFiles),
    GetFile(GetFile),
}

#[derive(Debug)]
pub struct CommandResponse {
    pub command_request_id: u64,
    pub result: CommandResult,
}

impl CommandResponse {
    pub fn new(command_request_id: u64, result: CommandResult) -> Self {
        Self {
            command_request_id,
            result,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    Register,
    ListPeers(Vec<String>),
    ListFiles(Vec<String>),
    GetFile {
        user_name: String,
        file_name: String,
    },
}

impl Command {
    pub fn new_register(user_name: String) -> Self {
        Self::Register(Register::new(user_name))
    }

    pub fn new_list_peers() -> Self {
        Self::ListPeers
    }

    pub fn new_list_files(user_name: String) -> Self {
        Self::ListFiles(ListFiles::new(user_name))
    }

    pub fn new_get_file(user_name: String, file_name: String) -> Self {
        Self::GetFile(GetFile::new(user_name, file_name))
    }
}

impl FromStr for Command {
    type Err = FileSharingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let command = parts.next().ok_or(FileSharingError::InvalidCommand)?;
        match command {
            "register" => {
                let user_name = parts.next().ok_or(FileSharingError::InvalidCommand)?;
                Ok(Command::Register(Register {
                    user_name: user_name.into(),
                }))
            }
            "ls_peers" => Ok(Command::ListPeers),
            str if str.starts_with("ls@") => {
                let user_name = str[3..]
                    .parse()
                    .map_err(|_| FileSharingError::InvalidCommand)?;
                Ok(Command::ListFiles(ListFiles { user_name }))
            }
            str if str.starts_with("get@") => {
                let mut parts = str[4..]
                    .split("->");
                let user_name = parts.next().ok_or_else(|| FileSharingError::InvalidCommand)?.into();
                let file_name = parts.next().ok_or_else(|| FileSharingError::InvalidCommand)?.into();
                Ok(Command::GetFile(GetFile { user_name, file_name }))
            }
            _ => Err(FileSharingError::InvalidCommand),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Register {
    pub user_name: String,
}

impl Register {
    pub fn new(user_name: String) -> Self {
        Self { user_name }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListFiles {
    pub user_name: String,
}

impl ListFiles {
    pub fn new(user_name: String) -> Self {
        Self { user_name }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetFile {
    pub user_name: String,
    pub file_name: String,
}

impl GetFile {
    pub fn new(user_name: String, file_name: String) -> Self {
        Self {
            user_name,
            file_name,
        }
    }
}
