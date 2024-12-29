// Copyright 2023 Remi Bernotavicius

use chrono::{offset::TimeZone as _, Local};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use nfs4::{FileAttribute, FileAttributeId, FileAttributes, FileHandle};
use nfs4_client::Result;
use std::net::TcpStream;
use std::path::PathBuf;
use hex::{FromHex, ToHex};

fn file_attrs(s: &str) -> std::result::Result<FileAttributes, String> {
    let mut attrs = FileAttributes::default();

    for e in s.split(',') {
        let i = e.find('=').ok_or(String::from("Missing `=`"))?;
        let key = &e[..i];
        let value = &e[(i + 1)..];
        attrs.insert(match key {
            "size" => FileAttribute::Size(value.parse::<u64>().map_err(|e| e.to_string())?),
            "owner" => FileAttribute::Owner(value.into()),
            "owner_group" => FileAttribute::OwnerGroup(value.into()),
            other => return Err(format!("unsupported attribute `{other}`")),
        });
    }

    Ok(attrs)
}

fn file_handle(s: &str) -> std::result::Result<FileHandle, String> {
    let fh = FileHandle(Vec::from_hex(&s).map_err(|e| e.to_string())?);
    Ok(fh)
}

#[derive(Subcommand)]
enum Command {
    GetAttr {
        path: PathBuf,
    },
    SetAttr {
        path: PathBuf,
        #[arg(value_parser = file_attrs)]
        attrs: FileAttributes,
    },
    ReadDir {
        path: PathBuf,
    },
    Remove {
        path: PathBuf,
    },
    Download {
        remote: PathBuf,
        local: PathBuf,
    },
    Upload {
        local: PathBuf,
        remote: PathBuf,
    },
    Ls {
        path: PathBuf,
    },
    LsFh {
        #[arg(value_parser = file_handle)]
        fh: FileHandle,
    },
    Cat {
        #[arg(value_parser = file_handle)]
        fh: FileHandle,
    }
}

#[derive(Parser)]
struct Options {
    host: String,
    #[clap(default_value_t = nfs4_client::NFS_PORT)]
    port: u16,
    #[command(subcommand)]
    command: Command,
}

fn print_listing(entries: &[nfs4::DirectoryEntry]) {
    for e in entries {
        let name = &e.name;
        let mode: &nfs4::Mode = e.attrs.get_as(FileAttributeId::Mode).unwrap();
        let num_links: &u32 = e.attrs.get_as(FileAttributeId::NumLinks).unwrap();
        let owner: &String = e.attrs.get_as(FileAttributeId::Owner).unwrap();
        let size: &u64 = e.attrs.get_as(FileAttributeId::Size).unwrap();

        let modify_raw: &nfs4::Time = e.attrs.get_as(FileAttributeId::TimeModify).unwrap();
        let modify = modify_raw.to_date_time().unwrap();
        let modify_str = Local.from_local_datetime(&modify).unwrap().to_rfc2822();

        println!("{mode:?} {num_links:3} {owner:5} {size:10} {modify_str:31} {name}");
    }
}

struct Cli {
    client: nfs4_client::Client<TcpStream>,
}

impl Cli {
    fn get_attr(&mut self, path: PathBuf) -> Result<()> {
        let handle = self.client.look_up(&path)?;
        let reply = self.client.get_attr(handle)?;
        println!("{reply:#?}");
        Ok(())
    }

    fn read_dir(&mut self, path: PathBuf) -> Result<()> {
        let handle = self.client.look_up(&path)?;
        let attr_request = [
            FileAttributeId::Mode,
            FileAttributeId::NumLinks,
            FileAttributeId::Owner,
            FileAttributeId::Size,
            FileAttributeId::TimeModify,
        ]
        .into_iter()
        .collect();
        let reply = self.client.read_dir(handle, attr_request)?;
        print_listing(&reply);
        Ok(())
    }

    fn remove(&mut self, path: PathBuf) -> Result<()> {
        let (parent_dir, name) = (path.parent().unwrap(), path.file_name().unwrap());
        let parent = self.client.look_up(parent_dir)?;
        self.client.remove(parent, name.to_str().unwrap())?;
        Ok(())
    }

    fn download(&mut self, remote: PathBuf, local: PathBuf) -> Result<()> {
        let local_file = if local.to_string_lossy().ends_with('/') {
            local.join(remote.file_name().unwrap())
        } else {
            local
        };

        let handle = self.client.look_up(&remote)?;
        let mut remote_attrs = self.client.get_attr(handle.clone())?.object_attributes;
        let size = remote_attrs.remove_as(FileAttributeId::Size).unwrap();

        let progress = ProgressBar::new(size).with_style(
            ProgressStyle::with_template("{wide_bar} {percent}% {binary_bytes_per_sec}").unwrap(),
        );
        let file = std::fs::File::create(local_file)?;
        self.client.read_all(handle, progress.wrap_write(file))?;
        Ok(())
    }

    fn set_attr(&mut self, path: PathBuf, attrs: FileAttributes) -> Result<()> {
        let handle = self.client.look_up(&path)?;
        self.client.set_attr(handle, attrs)?;
        Ok(())
    }

    fn upload(&mut self, local: PathBuf, remote: PathBuf) -> Result<()> {
        let (parent_dir, name) = if remote.to_string_lossy().ends_with('/') {
            (remote.as_ref(), local.file_name().unwrap())
        } else {
            (remote.parent().unwrap(), remote.file_name().unwrap())
        };

        let parent = self.client.look_up(parent_dir)?;
        let handle = self.client.create_file(parent, name.to_str().unwrap())?;

        let file = std::fs::File::open(local)?;
        let progress = ProgressBar::new(file.metadata()?.len()).with_style(
            ProgressStyle::with_template("{wide_bar} {percent}% {binary_bytes_per_sec}").unwrap(),
        );
        self.client.write_all(handle, progress.wrap_read(file))?;
        Ok(())
    }

    fn ls(&mut self, path: PathBuf) -> Result<()> {
        let handle = self.client.look_up(&path)?;

        let attr_request = [
            FileAttributeId::Mode,
            FileAttributeId::NumLinks,
            FileAttributeId::Owner,
            FileAttributeId::Size,
            FileAttributeId::TimeModify,
            FileAttributeId::FileHandle,
        ]
        .into_iter()
        .collect();
        let reply = self.client.read_dir(handle, attr_request)?;
        for e in reply {
            let name = &e.name;
            let fh: &FileHandle = e.attrs.get_as(FileAttributeId::FileHandle).unwrap();
            let fhstr: String = fh.0.encode_hex();
            println!("{fhstr} {name}");
        }

        Ok(())
    }

    fn lsfh(&mut self, fh: FileHandle) -> Result<()> {
        let attr_request = [
            FileAttributeId::FileHandle,
        ]
        .into_iter()
        .collect();
        let reply = self.client.read_dir(fh, attr_request)?;
        for e in reply {
            let name = &e.name;
            let fh: &FileHandle = e.attrs.get_as(FileAttributeId::FileHandle).unwrap();
            let fhstr: String = fh.0.encode_hex();
            println!("{fhstr} {name}");
        }

        Ok(())
    }

    fn cat(&mut self, fh: FileHandle) -> Result<()> {
        self.client.read_all(fh, std::io::stdout())?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let opts = Options::parse();

    let transport = TcpStream::connect((opts.host, opts.port))?;
    let client = nfs4_client::Client::new(transport)?;

    let mut cli = Cli { client };
    match opts.command {
        Command::GetAttr { path } => cli.get_attr(path)?,
        Command::ReadDir { path } => cli.read_dir(path)?,
        Command::Remove { path } => cli.remove(path)?,
        Command::Download { remote, local } => cli.download(remote, local)?,
        Command::SetAttr { path, attrs } => cli.set_attr(path, attrs)?,
        Command::Upload { local, remote } => cli.upload(local, remote)?,
        Command::Ls { path } => cli.ls(path)?,
        Command::LsFh { fh } => cli.lsfh(fh)?,
        Command::Cat { fh } => cli.cat(fh)?,
    }

    Ok(())
}
