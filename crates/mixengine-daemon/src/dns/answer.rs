//! What this server answers, as a function of the question — roadmap task **T44**.
//!
//! **Pure, and deliberately so** (the T44 design, D5). Nothing here binds a socket, reads the
//! configuration or touches the database: a question goes in and a [`Reply`] comes out, so the
//! policy this module *is* can be asserted on as a table rather than by standing up a server and
//! sending it packets. `super::server` is what turns a [`Reply`] into bytes on a wire.
//!
//! The whole policy is two sentences. A name inside a TLD MixEngine manages resolves to loopback,
//! whether or not a site has been declared for it — which is what makes `site.create` cost no
//! elevation prompt, because a wildcard needs no per-name record. Everything else is `REFUSED`.
//!
//! # There is no forwarder here, on purpose
//!
//! The T44 design, D1: every resolver mechanism T45 can use is scoped to a TLD, so a query outside
//! one never arrives, and the single way it could — a Linux wiring that replaced a link's DNS
//! servers with ours — is also the one where forwarding loops back through `systemd-resolved` and
//! hangs. `REFUSED` sends a stub resolver to its next nameserver at once, which turns a mis-wiring
//! into something `mix doctor` can see rather than a machine that has become mysteriously slow.

use std::net::Ipv4Addr;
use std::sync::LazyLock;

use hickory_proto::op::{LowerQuery, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, NS, SOA};
use hickory_proto::rr::{DNSClass, LowerName, Name, RData, Record, RecordType};
use mixengine_proto::domains::MANAGED_TLDS;

/// The one address a managed name resolves to.
///
/// `127.0.0.1` alone and never `::1` — the T44 design, D3, which is
/// [`mixengine_core::hosts`]' answer to the same question for the same reason: after T43 the front
/// end binds IPv4 only, and a name that resolves to an address nothing is listening on is a browser
/// preferring IPv6 and waiting before it falls back, on every connection.
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// How long an answer may be cached, in seconds.
///
/// The address is a constant, so nothing about *it* goes stale. What can is the mechanism: a home
/// that falls back to hosts-only should not have resolvers holding these answers for an hour. A
/// minute bounds that window and still costs one query per name per minute.
const TTL: u32 = 60;

/// The name in the `MNAME` field of a synthesised [`SOA`], under each managed TLD.
///
/// It resolves to loopback like everything else under that TLD, so the record names something this
/// server will actually answer for rather than a host that does not exist.
const NAMESERVER_LABEL: &str = "mixengine";

/// The mailbox in the `RNAME` field. Never read by anything; present because the record has a slot.
const HOSTMASTER_LABEL: &str = "hostmaster";

/// What the server should send back.
///
/// Plain data rather than a `MessageResponse`: hickory's response type carries four iterator type
/// parameters and a lifetime tied to the request, which is exactly the shape a test cannot build by
/// hand. Assembling one is [`super::server`]'s job, on the far side of this decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reply {
    /// The response code.
    pub(super) code: ResponseCode,

    /// Whether to set `AA`. True exactly when this server is speaking for a zone it manages.
    pub(super) authoritative: bool,

    /// The answer section.
    pub(super) answers: Vec<Record>,

    /// The authority section: the `SOA` that makes a negative answer cacheable (RFC 2308), and
    /// nothing else.
    pub(super) authority: Vec<Record>,
}

impl Reply {
    /// A question this server will not take: `REFUSED`, no records, not authoritative.
    ///
    /// **The default for everything outside a managed TLD**, and it is a refusal rather than
    /// `NXDOMAIN` because those two say different things. `NXDOMAIN` is "I am authoritative here
    /// and this name does not exist", which would be a lie about somebody else's zone.
    fn refused() -> Self {
        Self {
            code: ResponseCode::Refused,
            authoritative: false,
            answers: Vec::new(),
            authority: Vec::new(),
        }
    }

    /// The name exists and has nothing of the type asked for.
    ///
    /// `NOERROR` with an empty answer section and the zone's `SOA` in the authority section, which
    /// is what lets a resolver cache the absence instead of asking again on every connection.
    fn no_data(zone: &Zone) -> Self {
        Self {
            code: ResponseCode::NoError,
            authoritative: true,
            answers: Vec::new(),
            authority: vec![zone.soa()],
        }
    }

    /// The wildcard answer: this name is here, on loopback.
    ///
    /// **`name` is the question's own spelling, not the lowered one this module matched on.** A
    /// resolver using DNS-0x20 randomises the case of the name it asks about and discards an answer
    /// whose owner name comes back in a different case; it is off by default in the resolvers that
    /// implement it, and echoing what was asked costs nothing.
    fn address(name: Name) -> Self {
        Self {
            code: ResponseCode::NoError,
            authoritative: true,
            answers: vec![Record::from_rdata(name, TTL, RData::A(A(LOOPBACK)))],
            authority: Vec::new(),
        }
    }

    /// A record answered at the apex of a zone this server manages.
    fn apex(record: Record) -> Self {
        Self {
            code: ResponseCode::NoError,
            authoritative: true,
            answers: vec![record],
            authority: Vec::new(),
        }
    }
}

/// One managed TLD, with the names its records are made of resolved once.
#[derive(Debug)]
struct Zone {
    /// `test.`, as a name.
    apex: Name,

    /// The same, for the suffix test.
    lower: LowerName,

    /// `mixengine.test.`
    nameserver: Name,

    /// `hostmaster.mixengine.test.`
    hostmaster: Name,
}

impl Zone {
    /// The zone's `SOA` over its own name, for the authority section of a negative answer.
    fn soa(&self) -> Record {
        Record::from_rdata(self.apex.clone(), TTL, RData::SOA(self.soa_data()))
    }

    /// The record itself, synthesised. Nothing here is served from a zone file, so the serial is 1
    /// and stays 1: there is no transfer to notice a change.
    fn soa_data(&self) -> SOA {
        SOA::new(
            self.nameserver.clone(),
            self.hostmaster.clone(),
            1,
            // Refresh, retry and expire are a secondary's instructions, and this zone has no
            // secondaries. They are filled with ordinary values rather than zeroes because a
            // zero in any of them is what some resolvers read as a malformed record.
            3_600,
            600,
            86_400,
            // The negative-caching TTL, which is the field that does something here.
            TTL,
        )
    }
}

/// Every managed TLD, resolved once.
///
/// [`MANAGED_TLDS`] is a compile-time table of ASCII labels, so each of these parses; an `expect`
/// here would fire on a table that had already broken every other user of it.
static ZONES: LazyLock<Vec<Zone>> = LazyLock::new(|| {
    MANAGED_TLDS
        .iter()
        .map(|tld| {
            let apex = Name::from_ascii(format!("{tld}.")).expect("a managed TLD is a label");
            let nameserver = Name::from_ascii(format!("{NAMESERVER_LABEL}.{tld}."))
                .expect("a managed TLD is a label");
            let hostmaster =
                Name::from_ascii(format!("{HOSTMASTER_LABEL}.{NAMESERVER_LABEL}.{tld}."))
                    .expect("a managed TLD is a label");

            Zone {
                lower: LowerName::new(&apex),
                apex,
                nameserver,
                hostmaster,
            }
        })
        .collect()
});

/// The zone `name` belongs to, or [`None`] when it belongs to none of them.
///
/// **The comparison is by label and never by string**, which is [`LowerName::zone_of`]'s whole
/// point: `blog.test.evil.com` ends with the characters of a managed TLD and is emphatically not
/// inside one.
fn zone_of(name: &LowerName) -> Option<&'static Zone> {
    ZONES.iter().find(|zone| zone.lower.zone_of(name))
}

/// What to answer, given the opcode and the questions a request carried.
///
/// A slice rather than one question because "exactly one" is itself part of the policy: a message
/// carrying none, or several, is refused here rather than being unwrapped by the caller.
pub(super) fn reply(op_code: OpCode, queries: &[LowerQuery]) -> Reply {
    // An update, a notify or a status is not a question this server has an answer to. Refusing is
    // also what keeps a `PrivilegedOp`-free daemon free of one: there is no zone to update.
    if op_code != OpCode::Query {
        return Reply::refused();
    }

    let [query] = queries else {
        return Reply::refused();
    };

    // `CH` and `HS` exist and are not what a browser asks in. `ANY` as a *class* is not a question
    // either.
    if query.query_class() != DNSClass::IN {
        return Reply::refused();
    }

    let Some(zone) = zone_of(query.name()) else {
        return Reply::refused();
    };

    let at_apex = query.name() == &zone.lower;

    // The name as it was written on the wire — see [`Reply::address`] for why the answer echoes it
    // rather than the lowered form the match above is made on.
    let asked = query.original().name().clone();

    match query.query_type() {
        // The wildcard, at any depth, including the apex itself.
        RecordType::A => Reply::address(asked),

        // Answered at the apex only: `NS` and `SOA` are statements about a zone, and `blog.test` is
        // not one. Below the apex they are NODATA like everything else.
        RecordType::SOA if at_apex => {
            Reply::apex(Record::from_rdata(asked, TTL, RData::SOA(zone.soa_data())))
        }
        RecordType::NS if at_apex => Reply::apex(Record::from_rdata(
            asked,
            TTL,
            RData::NS(NS(zone.nameserver.clone())),
        )),

        // `AAAA` lands here with everything else, and that is the decision — see [`LOOPBACK`].
        _ => Reply::no_data(zone),
    }
}

#[cfg(test)]
mod tests {
    use hickory_proto::op::Query;

    use super::*;

    /// The question a test asks, as a one-element slice.
    fn ask(name: &str, record_type: RecordType) -> Vec<LowerQuery> {
        let mut query = Query::new();
        query
            .set_name(Name::from_ascii(name).expect("a name"))
            .set_query_type(record_type);

        vec![LowerQuery::query(query)]
    }

    /// The same, in a class nobody browses in.
    fn ask_in_class(name: &str, class: DNSClass) -> Vec<LowerQuery> {
        let mut query = Query::new();
        query
            .set_name(Name::from_ascii(name).expect("a name"))
            .set_query_type(RecordType::A)
            .set_query_class(class);

        vec![LowerQuery::query(query)]
    }

    fn answered(reply: &Reply) -> Vec<RData> {
        reply
            .answers
            .iter()
            .map(|record| record.data.clone())
            .collect()
    }

    /// The wildcard, which is the whole feature: any depth, any label, no site required.
    #[test]
    fn every_name_under_a_managed_tld_is_loopback() {
        for name in [
            "test.",
            "blog.test.",
            "api.blog.test.",
            "a.very.deep.name.under.blog.test.",
            "shop.localhost.",
            "printer.local.",
        ] {
            let reply = reply(OpCode::Query, &ask(name, RecordType::A));

            assert_eq!(reply.code, ResponseCode::NoError, "{name}");
            assert!(reply.authoritative, "{name}");
            assert_eq!(
                answered(&reply),
                vec![RData::A(A(Ipv4Addr::LOCALHOST))],
                "{name}"
            );
            assert_eq!(reply.answers[0].ttl, TTL, "{name}");
        }
    }

    /// A name is matched by label, so a suffix that merely *reads* like a managed TLD is not one.
    /// This is the row that would be wrong if the check were ever written with `ends_with`.
    #[test]
    fn a_managed_tld_in_the_middle_of_a_public_name_is_not_a_match() {
        for name in [
            "blog.test.evil.com.",
            "test.example.com.",
            "notatest.",
            "example.com.",
            "localhost.example.com.",
        ] {
            let reply = reply(OpCode::Query, &ask(name, RecordType::A));

            assert_eq!(reply.code, ResponseCode::Refused, "{name}");
            assert!(!reply.authoritative, "{name}");
            assert!(reply.answers.is_empty(), "{name}");
        }
    }

    /// A name that arrives without its trailing dot is the same name. Everything off a wire is
    /// fully qualified, so this is a guard on the helper the tests themselves are written with.
    #[test]
    fn a_name_without_its_trailing_dot_is_the_same_name() {
        assert_eq!(
            reply(OpCode::Query, &ask("blog.test", RecordType::A)).code,
            ResponseCode::NoError
        );
    }

    /// DNS is case-insensitive, and the answer's owner name keeps the case that was asked.
    ///
    /// The second half is not cosmetic: a resolver using DNS-0x20 randomises the case it asks in
    /// and **discards** an answer that comes back spelled differently.
    #[test]
    fn a_name_in_capitals_is_the_same_name_and_is_answered_in_capitals() {
        let reply = reply(OpCode::Query, &ask("BlOg.TeSt.", RecordType::A));

        assert_eq!(reply.code, ResponseCode::NoError);
        assert_eq!(answered(&reply), vec![RData::A(A(Ipv4Addr::LOCALHOST))]);
        assert_eq!(
            reply.answers[0].name.to_string(),
            "BlOg.TeSt.",
            "an answer spelled differently from the question is dropped by a 0x20 resolver"
        );
    }

    /// `AAAA` is NOERROR with nothing in it, and an `SOA` so the absence can be cached — D3 and
    /// D10. Answering `::1` would be answering with an address nothing binds.
    #[test]
    fn aaaa_says_the_name_exists_and_has_no_ipv6_address() {
        let reply = reply(OpCode::Query, &ask("blog.test.", RecordType::AAAA));

        assert_eq!(reply.code, ResponseCode::NoError);
        assert!(reply.authoritative);
        assert!(reply.answers.is_empty());
        assert!(
            matches!(reply.authority.as_slice(), [record] if matches!(record.data, RData::SOA(_))),
            "a negative answer without an SOA is re-asked on every connection"
        );
    }

    /// Everything that is not `A` under a managed name gets the same shape as `AAAA`.
    #[test]
    fn every_other_type_under_a_managed_name_is_no_data() {
        for record_type in [
            RecordType::MX,
            RecordType::TXT,
            RecordType::SRV,
            RecordType::CNAME,
            RecordType::PTR,
            RecordType::CAA,
        ] {
            let reply = reply(OpCode::Query, &ask("blog.test.", record_type));

            assert_eq!(reply.code, ResponseCode::NoError, "{record_type}");
            assert!(reply.answers.is_empty(), "{record_type}");
            assert_eq!(reply.authority.len(), 1, "{record_type}");
        }
    }

    /// `SOA` and `NS` are statements about a zone, so they are answered at the apex and nowhere
    /// else.
    #[test]
    fn soa_and_ns_are_answered_at_the_apex_only() {
        let soa = reply(OpCode::Query, &ask("test.", RecordType::SOA));
        assert!(
            matches!(answered(&soa).as_slice(), [RData::SOA(_)]),
            "{soa:?}"
        );

        let ns = reply(OpCode::Query, &ask("test.", RecordType::NS));
        assert!(matches!(answered(&ns).as_slice(), [RData::NS(_)]), "{ns:?}");

        for below in ["blog.test.", "api.blog.test."] {
            let reply = reply(OpCode::Query, &ask(below, RecordType::SOA));

            assert!(reply.answers.is_empty(), "{below}");
            assert_eq!(reply.code, ResponseCode::NoError, "{below}");
        }
    }

    /// The nameserver an `NS` names resolves here too, rather than pointing at a host that does not
    /// exist.
    #[test]
    fn the_nameserver_a_zone_names_answers_on_loopback() {
        let reply = reply(
            OpCode::Query,
            &ask(&format!("{NAMESERVER_LABEL}.test."), RecordType::A),
        );

        assert_eq!(answered(&reply), vec![RData::A(A(Ipv4Addr::LOCALHOST))]);
    }

    /// A message with no question, or with several, is not a question this server answers.
    #[test]
    fn a_message_without_exactly_one_question_is_refused() {
        assert_eq!(reply(OpCode::Query, &[]).code, ResponseCode::Refused);

        let mut two = ask("blog.test.", RecordType::A);
        two.extend(ask("api.blog.test.", RecordType::A));

        assert_eq!(reply(OpCode::Query, &two).code, ResponseCode::Refused);
    }

    /// Everything that is not a query, and everything that is not `IN`.
    #[test]
    fn an_opcode_or_a_class_this_server_does_not_speak_is_refused() {
        for op_code in [OpCode::Update, OpCode::Notify, OpCode::Status] {
            assert_eq!(
                reply(op_code, &ask("blog.test.", RecordType::A)).code,
                ResponseCode::Refused,
                "{op_code:?}"
            );
        }

        for class in [DNSClass::CH, DNSClass::HS, DNSClass::NONE] {
            assert_eq!(
                reply(OpCode::Query, &ask_in_class("blog.test.", class)).code,
                ResponseCode::Refused,
                "{class}"
            );
        }
    }

    /// The table this server answers for is `mixengine-proto`'s, not a second copy of it. A TLD
    /// added there without a thought here would otherwise go unanswered.
    #[test]
    fn every_managed_tld_has_a_zone_and_nothing_else_does() {
        assert_eq!(ZONES.len(), MANAGED_TLDS.len());

        for tld in MANAGED_TLDS {
            let name =
                LowerName::new(&Name::from_ascii(format!("anything.{tld}.")).expect("a name"));

            assert!(zone_of(&name).is_some(), "{tld}");
        }
    }
}
