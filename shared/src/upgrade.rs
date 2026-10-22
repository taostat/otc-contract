use ink::primitives::Hash;

/// Identifies which contract a code hash in the code registry belongs to.
///
/// Kinds are plain numbers rather than an enum so the registry, which is never
/// upgraded, can hold code for contract kinds added later.
pub type CodeKind = u16;

pub const LISTING_VAULT: CodeKind = 1;
pub const ESCROW: CodeKind = 2;
pub const LOCKUP_FACTORY: CodeKind = 3;
pub const OTC: CodeKind = 4;

/// Who may upgrade a position contract (a listing vault or an escrow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
pub enum UpgradePolicy {
    /// The position owner can adopt the registry's current code at any time, and
    /// anyone can apply it once it has been current for the force delay.
    #[default]
    Auto,
    /// Only the position owner can upgrade the position.
    ConsentOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
pub enum UpgradeError {
    /// The registry has no current code for this kind
    NoCurrentCode,
    /// The requested code is not the registry's current code for this kind
    NotCurrentCode,
    /// Only the position owner may upgrade this position
    ConsentRequired,
    /// The current code has not been current for the force delay yet
    ForceDelayNotElapsed,
}

/// Decide whether `requested` may be installed on a position.
///
/// `current` and `force_target` come from the code registry for the position's
/// kind: the current code, and the same code once it has been current for the
/// force delay. A position only ever moves to the current code. Its owner may
/// adopt it immediately; anyone else may apply it after the force delay, unless
/// the owner chose `ConsentOnly`.
pub fn check_upgrade(
    caller_is_owner: bool,
    policy: UpgradePolicy,
    requested: Hash,
    current: Option<Hash>,
    force_target: Option<Hash>,
) -> Result<(), UpgradeError> {
    let current = current.ok_or(UpgradeError::NoCurrentCode)?;
    if requested != current {
        return Err(UpgradeError::NotCurrentCode);
    }

    if caller_is_owner {
        return Ok(());
    }

    if policy == UpgradePolicy::ConsentOnly {
        return Err(UpgradeError::ConsentRequired);
    }

    match force_target {
        Some(target) if target == requested => Ok(()),
        _ => Err(UpgradeError::ForceDelayNotElapsed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> Hash {
        Hash::from([byte; 32])
    }

    #[test]
    fn upgrade_rules() {
        let a = hash(1);
        let b = hash(2);

        struct Case {
            name: &'static str,
            caller_is_owner: bool,
            policy: UpgradePolicy,
            requested: Hash,
            current: Option<Hash>,
            force_target: Option<Hash>,
            expected: Result<(), UpgradeError>,
        }

        let cases = [
            Case {
                name: "owner adopts current code before the force delay",
                caller_is_owner: true,
                policy: UpgradePolicy::Auto,
                requested: a,
                current: Some(a),
                force_target: None,
                expected: Ok(()),
            },
            Case {
                name: "owner adopts current code under ConsentOnly",
                caller_is_owner: true,
                policy: UpgradePolicy::ConsentOnly,
                requested: a,
                current: Some(a),
                force_target: None,
                expected: Ok(()),
            },
            Case {
                name: "anyone applies current code after the force delay",
                caller_is_owner: false,
                policy: UpgradePolicy::Auto,
                requested: a,
                current: Some(a),
                force_target: Some(a),
                expected: Ok(()),
            },
            Case {
                name: "anyone is refused before the force delay",
                caller_is_owner: false,
                policy: UpgradePolicy::Auto,
                requested: a,
                current: Some(a),
                force_target: None,
                expected: Err(UpgradeError::ForceDelayNotElapsed),
            },
            Case {
                name: "anyone is refused under ConsentOnly even after the delay",
                caller_is_owner: false,
                policy: UpgradePolicy::ConsentOnly,
                requested: a,
                current: Some(a),
                force_target: Some(a),
                expected: Err(UpgradeError::ConsentRequired),
            },
            Case {
                name: "nobody installs code that is not current",
                caller_is_owner: true,
                policy: UpgradePolicy::Auto,
                requested: b,
                current: Some(a),
                force_target: Some(a),
                expected: Err(UpgradeError::NotCurrentCode),
            },
            Case {
                name: "nobody upgrades when the kind has no current code",
                caller_is_owner: true,
                policy: UpgradePolicy::Auto,
                requested: a,
                current: None,
                force_target: None,
                expected: Err(UpgradeError::NoCurrentCode),
            },
        ];

        for case in cases {
            assert_eq!(
                check_upgrade(
                    case.caller_is_owner,
                    case.policy,
                    case.requested,
                    case.current,
                    case.force_target,
                ),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn auto_is_the_default_policy() {
        assert_eq!(UpgradePolicy::default(), UpgradePolicy::Auto);
    }
}
