// Copyright Remi Bernotavicius

use nfs4::{FileAttribute, FileAttributeId, FileHandle};
use nfs4_client::Client;
use nfs4_client::NFS_PORT;
use std::collections::BTreeSet;
use std::net::TcpStream;
use std::path::Path;

macro_rules! test {
    ($test_name:ident) => {
        (Self::$test_name as fn(&mut Self), stringify!($test_name))
    };
}

struct Fixture<'machine> {
    machine: &'machine mut vm_runner::Machine,
    client: Client,
    transport: TcpStream,
}

impl<'machine> Fixture<'machine> {
    fn new(machine: &'machine mut vm_runner::Machine) -> Self {
        let port = machine
            .forwarded_ports()
            .iter()
            .find(|p| p.guest == NFS_PORT)
            .unwrap();
        let mut transport = TcpStream::connect(("127.0.0.1", port.host)).unwrap();
        let client = Client::new(&mut transport).unwrap();

        Self {
            machine,
            client,
            transport,
        }
    }

    fn run(&mut self) {
        let tests = [
            test!(create_file_test),
            test!(read_write_test),
            test!(set_attr_test),
            test!(read_dir_test),
            test!(remove_test),
        ];

        for (test, test_name) in tests {
            log::info!("running test {}:Fixture::{}", file!(), test_name);
            test(self);
            self.machine.run_command("rm -rf /files/*");
            self.machine.run_command("ls /files/"); // XXX its not waiting lol
        }
    }

    //  _          _
    // | |__   ___| |_ __   ___ _ __ ___
    // | '_ \ / _ \ | '_ \ / _ \ '__/ __|
    // | | | |  __/ | |_) |  __/ |  \__ \
    // |_| |_|\___|_| .__/ \___|_|  |___/
    //              |_|

    fn get_file_size(&mut self, path: &str) -> u64 {
        let reply = self.client.get_attr(&mut self.transport, path).unwrap();
        *reply
            .object_attributes
            .get_as::<u64>(FileAttributeId::Size)
            .unwrap()
    }

    fn create_file(&mut self, path: impl AsRef<Path>) -> FileHandle {
        let path = path.as_ref();

        let parent = self
            .client
            .look_up(&mut self.transport, path.parent().unwrap())
            .unwrap();
        self.client
            .create_file(
                &mut self.transport,
                parent.clone(),
                path.file_name().unwrap().to_str().unwrap(),
            )
            .unwrap()
    }

    fn create_file_test(&mut self) {
        self.create_file("/files/a_file");
        self.client
            .look_up(&mut self.transport, "/files/a_file")
            .unwrap();
    }

    //  _            _
    // | |_ ___  ___| |_ ___
    // | __/ _ \/ __| __/ __|
    // | ||  __/\__ \ |_\__ \
    //  \__\___||___/\__|___/
    //

    fn read_write_test(&mut self) {
        let handle = self.create_file("/files/a_file");

        let test_contents: Vec<u8> = (0..100_000).map(|v| (v % 255) as u8).collect();
        self.client
            .write_all(&mut self.transport, handle.clone(), &test_contents[..])
            .unwrap();

        let mut read_data = vec![];
        self.client
            .read_all(&mut self.transport, handle.clone(), &mut read_data)
            .unwrap();
        assert_eq!(read_data, test_contents);

        assert_eq!(self.get_file_size("/files/a_file"), read_data.len() as u64);
    }

    fn set_attr_test(&mut self) {
        let handle = self.create_file("/files/a_file");

        self.client
            .set_attr(
                &mut self.transport,
                handle,
                [FileAttribute::Size(100)].into_iter().collect(),
            )
            .unwrap();

        let reply = self
            .client
            .get_attr(&mut self.transport, "/files/a_file")
            .unwrap();
        assert_eq!(
            *reply
                .object_attributes
                .get_as::<u64>(FileAttributeId::Size)
                .unwrap(),
            100
        );
    }

    fn read_dir_test(&mut self) {
        let parent = self.client.look_up(&mut self.transport, "/files").unwrap();

        let mut expected = BTreeSet::new();

        for i in 0..100 {
            let name = format!("a_file{i}");
            self.client
                .create_file(&mut self.transport, parent.clone(), &name)
                .unwrap();
            expected.insert(name);
        }

        let entries = self
            .client
            .read_dir(&mut self.transport, parent.clone())
            .unwrap();
        let actual: BTreeSet<String> = entries.into_iter().map(|e| e.name).collect();
        assert_eq!(actual, expected);
    }

    fn remove_test(&mut self) {
        self.create_file("/files/a_file");
        let parent = self.client.look_up(&mut self.transport, "/files").unwrap();

        self.client
            .remove(&mut self.transport, parent, "a_file")
            .unwrap();
        self.client
            .look_up(&mut self.transport, "/files/a_file")
            .unwrap_err();
    }
}

#[test]
fn linux_server() {
    vm_test_fixture::fixture(&[NFS_PORT], |m| {
        let mut fix = Fixture::new(m);
        fix.run();
    });
}
