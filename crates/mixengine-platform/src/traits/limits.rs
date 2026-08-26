//! What this machine will actually enforce of a service's declared limits.

/// How this system caps a supervised service. One per system, chosen by the system.
///
/// The same shape as [`PortAccessMethod`](crate::PortAccessMethod) and for its reason: there is no
/// fallback chain and nothing to negotiate, because a machine has the mechanism its operating
/// system has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitMechanism {
    /// A job object — which is the group a supervised child is already in. Windows.
    JobObject,

    /// A cgroup v2 under a subtree delegated to this session. Linux.
    CgroupV2,

    /// Nothing caps anything. macOS.
    None,
}

/// What walking into a cap does to the service.
///
/// **Reported rather than left to be assumed**, because the two endings need different words in
/// front of a person: a program handed a failed allocation usually says something first, and one the
/// kernel kills says nothing at all. A client that offered "limit to 512 MB" without saying which of
/// the two happens would be telling half the truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenExceeded {
    /// The next allocation fails, and the program is handed the failure. Windows.
    AllocationFails,

    /// The kernel reclaims first, and kills something inside the cap when reclaiming is not
    /// enough. Linux.
    Killed,
}

/// What one field of a declared `ResourceLimits` actually does here.
///
/// **Per field rather than one flag for the pair, and Linux is why.** systemd delegates a user
/// session's `memory` controller far more readily than its `cpu` controller, and which of the two
/// arrives has moved between releases — so a machine that caps memory and cannot cap CPU is an
/// ordinary machine, and a single flag could only describe it by lying about one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// A wall. [`when`](Self::Hard::when) says what walking into it does.
    Hard {
        /// What the service sees at the cap.
        when: WhenExceeded,
    },

    /// This operating system has no mechanism for this field, and no release will add one.
    ///
    /// **Different advice from [`Unavailable`](Self::Unavailable)**, which is the whole reason the
    /// two are separate variants: a client should not draw this control at all, and `mix doctor`
    /// should say nothing about it — a permanent fact about an operating system is not news about
    /// this machine.
    Unsupported,

    /// The mechanism exists on this system and this machine will not lend it.
    ///
    /// Fixable, in principle, by changing how the session is started — which is why this one is
    /// worth a sentence and [`Unsupported`](Self::Unsupported) is not.
    Unavailable {
        /// Why not, phrased for a person: this is the line `mix doctor` prints.
        why: String,
    },
}

/// What a memory limit is measured as here.
///
/// One number, two meanings, and neither system is wrong: a job object bounds **commit charge**, and
/// cgroup v2's `memory.max` bounds **charged pages**, which includes page cache. Reported beside the
/// number rather than resolved into one of them, so a caller is told what it means *here* instead of
/// assuming it means the same everywhere — which is [`PortBinding`](crate::PortBinding)'s idea
/// applied to a quantity rather than to a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMeasure {
    /// Everything the process has asked the system to promise it. Windows.
    Commit,

    /// Anonymous memory plus the page cache charged to the group. Linux.
    ChargedPages,
}

/// Everything a client needs to know before it offers a limit control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitSupport {
    /// Which mechanism this system uses.
    pub mechanism: LimitMechanism,

    /// What a `cpu_percent` does here.
    pub cpu: Enforcement,

    /// What a `memory_mb` does here.
    pub memory: Enforcement,

    /// What a `memory_mb` is measured as here.
    pub memory_measure: MemoryMeasure,

    /// Whether asking for a background priority does anything.
    ///
    /// True on all three so far, and reported rather than assumed so that the day one of them
    /// cannot, nothing above has to change shape to say so.
    pub priority: bool,

    /// How many cores a `cpu_percent` may be spent across.
    ///
    /// **So a client can draw the ceiling it would otherwise be refused at**: `cpu_percent` is a
    /// percentage of *one* core, so 800 is the whole of an eight-core machine. The refusal itself
    /// lives in the daemon rather than in `mixengine-proto`, because this number is a property of
    /// the machine and proto has no host to ask.
    pub cores: u32,
}

/// What this machine will enforce of a service's declared limits.
///
/// **Reads only.** Applying a limit is not a question asked of the machine: it happens to a
/// particular child, through the handle that spawned it, at the moment it is spawned or while it
/// runs — and that lives in [`process`](crate::process). The same split
/// [`PortAccess`](crate::PortAccess) makes, for the same reason.
pub trait ResourceControl: std::fmt::Debug + Send + Sync {
    /// What this machine will enforce, field by field.
    fn support(&self) -> LimitSupport;
}
