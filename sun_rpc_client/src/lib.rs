// Copyright 2023 Remi Bernotavicius

use derive_more::From;
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, io};
use sun_rpc::{
    AcceptedReplyBody, AuthSysParameters, CallBody, Gid, Message, MessageBody, OpaqueAuth,
    ReplyBody, Uid, Xid,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, From)]
pub enum Error {
    Deseralization(serde_xdr::CompatDeserializationError),
    Serialization(serde_xdr::CompatSerializationError),
    Io(io::Error),
    ProgramUnavailable,
    ProgramMismatch,
    ProcedureUnavailable,
    GarbageArguments,
    SystemError,
    #[from(ignore)]
    UnexpectedReply(String),
}

pub trait Transport: io::Read + io::Write {}

impl<T> Transport for T where T: io::Read + io::Write {}

pub const PORT_MAPPER: u32 = 100000;
pub const PORT_MAPPER_PORT: u16 = 111;
pub const NULL_PROCEDURE: u32 = 0;

pub struct RpcClient<TransportT> {
    xid: Xid,
    program: u32,
    transport: TransportT,
}

impl<TransportT: Transport> RpcClient<TransportT> {
    pub fn new(transport: TransportT, program: u32) -> Self {
        Self {
            xid: Xid(1),
            program,
            transport,
        }
    }

    pub fn send_request<T: Serialize>(&mut self, procedure: u32, call_args: T) -> Result<()> {
        let message = Message {
            xid: self.xid.clone(),
            body: MessageBody::Call(CallBody {
                rpc_version: 2,
                program: self.program,
                version: 4,
                procedure,
                credential: OpaqueAuth::auth_sys(AuthSysParameters {
                    stamp: 0,
                    machine_name: "test-machine".into(),
                    uid: Uid(1337),
                    gid: Gid(42),
                    gids: vec![Gid(1337)],
                }),
                verifier: OpaqueAuth::none(),
                call_args,
            }),
        };
        let mut serialized = vec![0; 4];
        serde_xdr::to_writer(&mut serialized, &message)?;

        let fragment_header = (serialized.len() - 4) as u32 | 0x1 << 31;
        serde_xdr::to_writer(&mut &mut serialized[..4], &fragment_header)?;

        self.transport.write_all(&serialized[..])?;

        self.xid = Xid(self.xid.0 + 1);

        Ok(())
    }

    pub fn receive_reply<T: DeserializeOwned + fmt::Debug>(&mut self) -> Result<T> {
        let fragment_header: u32 = serde_xdr::from_reader(&mut self.transport)?;
        let length = fragment_header & !(0x1 << 31);
        let reply: Message<T> =
            serde_xdr::from_reader(&mut io::Read::take(&mut self.transport, length as u64))?;

        if let Message {
            body: MessageBody::Reply(ReplyBody::Accepted(accepted_reply)),
            ..
        } = reply
        {
            match accepted_reply.body {
                AcceptedReplyBody::Success(b) => Ok(b),
                AcceptedReplyBody::ProgramUnavailable => Err(Error::ProgramUnavailable),
                AcceptedReplyBody::ProgramMismatch { .. } => Err(Error::ProgramMismatch),
                AcceptedReplyBody::ProcedureUnavailable => Err(Error::ProcedureUnavailable),
                AcceptedReplyBody::GarbageArguments => Err(Error::GarbageArguments),
                AcceptedReplyBody::SystemError => Err(Error::SystemError),
            }
        } else {
            Err(Error::UnexpectedReply(format!("{reply:?}")))
        }
    }
}

#[test]
fn ping() {
    vm_test_fixture::fixture(&[PORT_MAPPER_PORT], |m| {
        let port = m
            .forwarded_ports()
            .iter()
            .find(|p| p.guest == PORT_MAPPER_PORT)
            .unwrap();
        let transport = std::net::TcpStream::connect(("127.0.0.1", port.host)).unwrap();
        let mut client = RpcClient::new(transport, PORT_MAPPER);

        client.send_request(NULL_PROCEDURE, ()).unwrap();

        client.receive_reply::<()>().unwrap();
    });
}
