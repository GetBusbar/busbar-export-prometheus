// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use std::sync::Arc;

use busbar_contract::abi::host::conn::connector::DIRECTION_INBOUND;
use busbar_contract::abi::mechanism::rendering::ReadNeed;
use busbar_contract::conn::Ticket;
use busbar_contract::conn::{
    ConnError, ConnId, Conns, DeclaredConns, InstanceId, NeedId, OpenDesc, Piece,
};
use busbar_contract::transport::ConnFacts;

/// This pull sink uses no connector I/O; the host still admits its declared HTTP listener.
pub struct Inbound;

impl Conns for Inbound {
    fn open(&self, _: InstanceId, _: NeedId, _: &OpenDesc<'_>) -> Result<ConnId, ConnError> {
        panic!("the scrape sink must not open an outbound connection")
    }
    fn write(
        &self,
        _: InstanceId,
        _: ConnId,
        _: &[u8],
        _: bool,
        _: bool,
    ) -> Result<usize, ConnError> {
        panic!("the scrape sink must not write to a connector")
    }
    fn read(&self, _: InstanceId, _: ConnId, _: Ticket, _: &mut [u8]) -> Result<Piece, ConnError> {
        panic!("the scrape sink must not read from a connector")
    }
    fn wait(&self, _: InstanceId, _: &[ConnId], _: Ticket) -> Result<usize, ConnError> {
        panic!("the scrape sink must not wait on a connector")
    }
    fn facts(&self, _: InstanceId, _: ConnId) -> Result<ConnFacts, ConnError> {
        panic!("the scrape sink has no connector facts")
    }
    fn close(&self, _: InstanceId, _: ConnId) -> Result<(), ConnError> {
        panic!("the scrape sink must not close a connector")
    }
}

impl DeclaredConns for Inbound {
    fn declare(
        &self,
        _: InstanceId,
        need: NeedId,
        spec: &ReadNeed,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<(), ConnError> {
        assert_eq!(need, NeedId(0));
        assert_eq!(spec.direction, DIRECTION_INBOUND);
        assert_eq!(spec.transport, "http");
        assert_eq!(spec.target_from, "listen");
        assert!(
            spec.auth.is_empty(),
            "the operator's data auth chain supplies auth"
        );
        Ok(())
    }
    fn declared(&self, _: InstanceId, need: NeedId) -> Option<Result<(), ConnError>> {
        (need == NeedId(0)).then_some(Ok(()))
    }
    fn serves_scheme(&self, scheme: &str) -> bool {
        scheme == "http"
    }
}

pub fn host(
    _: busbar_contract::conn::ConnWake,
    anchors: Option<&str>,
) -> busbar_contract::conn::admissions::AdmissionAuthority {
    assert!(anchors.is_none());
    busbar_contract::conn::admissions::AdmissionAuthority::issue(
        &busbar_contract::caps::KernelSeal::acquire_for_kernel(),
        Arc::new(Inbound),
        busbar_contract::secret_ref::Sugars::default(),
    )
}
