//! Team accounts with role-based access control.
//!
//! Teams allow multiple users to collaborate on shared deployments.
//! Each team has an owner, optional admins, and members. Permission
//! hierarchy: Owner > Admin > Member.

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// A team in the cloud platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub owner_user_id: String,
    pub created_at: u64,
    pub members: Vec<TeamMember>,
}

/// A member within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub user_id: String,
    pub role: TeamRole,
    pub joined_at: u64,
}

/// Role within a team, ordered by privilege level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
}

impl TeamRole {
    /// Numeric privilege level (higher = more privileged).
    fn privilege_level(self) -> u8 {
        match self {
            Self::Owner => 3,
            Self::Admin => 2,
            Self::Member => 1,
        }
    }

    /// Returns true if this role has at least the privilege of `required`.
    pub fn has_privilege(self, required: Self) -> bool {
        self.privilege_level() >= required.privilege_level()
    }
}

impl fmt::Display for TeamRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(f, "owner"),
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
        }
    }
}

/// Errors from team operations.
#[derive(Debug, Clone, Serialize)]
pub enum TeamError {
    TeamNotFound { team_id: String },
    UserNotMember { user_id: String },
    UserAlreadyMember { user_id: String },
    CannotRemoveOwner,
    CannotChangeOwnerRole,
    InsufficientPermission { required: String },
}

impl fmt::Display for TeamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TeamNotFound { team_id } => write!(f, "team not found: {team_id}"),
            Self::UserNotMember { user_id } => write!(f, "user is not a member: {user_id}"),
            Self::UserAlreadyMember { user_id } => {
                write!(f, "user is already a member: {user_id}")
            }
            Self::CannotRemoveOwner => write!(f, "cannot remove the team owner"),
            Self::CannotChangeOwnerRole => write!(f, "cannot change the owner's role"),
            Self::InsufficientPermission { required } => {
                write!(f, "insufficient permission, required: {required}")
            }
        }
    }
}

impl std::error::Error for TeamError {}

/// In-memory team store. Will be replaced with Postgres in production.
#[derive(Clone)]
pub struct TeamStore {
    teams: Arc<RwLock<HashMap<String, Team>>>,
}

impl TeamStore {
    pub fn new() -> Self {
        Self {
            teams: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new team. The creator becomes the owner.
    pub fn create_team(&self, name: &str, owner_user_id: &str) -> Team {
        let team_id = generate_team_id();
        let now = epoch_secs();

        let team = Team {
            id: team_id.clone(),
            name: name.to_string(),
            owner_user_id: owner_user_id.to_string(),
            created_at: now,
            members: vec![TeamMember {
                user_id: owner_user_id.to_string(),
                role: TeamRole::Owner,
                joined_at: now,
            }],
        };

        {
            let mut teams = self.teams.write().unwrap();
            teams.insert(team_id, team.clone());
        }

        team
    }

    /// Get a team by ID.
    pub fn get_team(&self, team_id: &str) -> Option<Team> {
        let teams = self.teams.read().unwrap();
        teams.get(team_id).cloned()
    }

    /// List all teams a user belongs to.
    pub fn list_teams_for_user(&self, user_id: &str) -> Vec<Team> {
        let teams = self.teams.read().unwrap();
        teams
            .values()
            .filter(|team| team.members.iter().any(|m| m.user_id == user_id))
            .cloned()
            .collect()
    }

    /// Add a member to a team.
    pub fn add_member(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<Team, TeamError> {
        let mut teams = self.teams.write().unwrap();
        let team = teams
            .get_mut(team_id)
            .ok_or_else(|| TeamError::TeamNotFound {
                team_id: team_id.to_string(),
            })?;

        if team.members.iter().any(|m| m.user_id == user_id) {
            return Err(TeamError::UserAlreadyMember {
                user_id: user_id.to_string(),
            });
        }

        // Cannot add a second owner.
        let actual_role = if role == TeamRole::Owner {
            TeamRole::Admin
        } else {
            role
        };

        team.members.push(TeamMember {
            user_id: user_id.to_string(),
            role: actual_role,
            joined_at: epoch_secs(),
        });

        Ok(team.clone())
    }

    /// Remove a member from a team. The owner cannot be removed.
    pub fn remove_member(&self, team_id: &str, user_id: &str) -> Result<Team, TeamError> {
        let mut teams = self.teams.write().unwrap();
        let team = teams
            .get_mut(team_id)
            .ok_or_else(|| TeamError::TeamNotFound {
                team_id: team_id.to_string(),
            })?;

        if team.owner_user_id == user_id {
            return Err(TeamError::CannotRemoveOwner);
        }

        let initial_len = team.members.len();
        team.members.retain(|m| m.user_id != user_id);

        if team.members.len() == initial_len {
            return Err(TeamError::UserNotMember {
                user_id: user_id.to_string(),
            });
        }

        Ok(team.clone())
    }

    /// Update a member's role. Cannot change the owner's role.
    pub fn update_role(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<Team, TeamError> {
        let mut teams = self.teams.write().unwrap();
        let team = teams
            .get_mut(team_id)
            .ok_or_else(|| TeamError::TeamNotFound {
                team_id: team_id.to_string(),
            })?;

        if team.owner_user_id == user_id {
            return Err(TeamError::CannotChangeOwnerRole);
        }

        let member = team
            .members
            .iter_mut()
            .find(|m| m.user_id == user_id)
            .ok_or_else(|| TeamError::UserNotMember {
                user_id: user_id.to_string(),
            })?;

        // Cannot promote someone to Owner through role update.
        let actual_role = if role == TeamRole::Owner {
            TeamRole::Admin
        } else {
            role
        };

        member.role = actual_role;
        Ok(team.clone())
    }

    /// Check if a user has at least the required role in a team.
    pub fn check_permission(
        &self,
        team_id: &str,
        user_id: &str,
        required_role: TeamRole,
    ) -> bool {
        let teams = self.teams.read().unwrap();
        let Some(team) = teams.get(team_id) else {
            return false;
        };

        team.members
            .iter()
            .find(|m| m.user_id == user_id)
            .map(|m| m.role.has_privilege(required_role))
            .unwrap_or(false)
    }
}

/// Generate a random team ID.
fn generate_team_id() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.r#gen();
    format!("team_{}", hex::encode(bytes))
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_team_sets_owner() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        assert!(team.id.starts_with("team_"));
        assert_eq!(team.name, "Acme");
        assert_eq!(team.owner_user_id, "usr_alice");
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].user_id, "usr_alice");
        assert_eq!(team.members[0].role, TeamRole::Owner);
    }

    #[test]
    fn add_member_and_list() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        let updated = store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .unwrap();
        assert_eq!(updated.members.len(), 2);

        let bob = updated.members.iter().find(|m| m.user_id == "usr_bob").unwrap();
        assert_eq!(bob.role, TeamRole::Member);

        // List teams for bob.
        let bob_teams = store.list_teams_for_user("usr_bob");
        assert_eq!(bob_teams.len(), 1);
        assert_eq!(bob_teams[0].id, team.id);
    }

    #[test]
    fn add_duplicate_member_fails() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .unwrap();

        let err = store
            .add_member(&team.id, "usr_bob", TeamRole::Admin)
            .unwrap_err();
        assert!(matches!(err, TeamError::UserAlreadyMember { .. }));
    }

    #[test]
    fn permission_hierarchy() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_owner");

        store
            .add_member(&team.id, "usr_admin", TeamRole::Admin)
            .unwrap();
        store
            .add_member(&team.id, "usr_member", TeamRole::Member)
            .unwrap();

        // Owner has all permissions.
        assert!(store.check_permission(&team.id, "usr_owner", TeamRole::Owner));
        assert!(store.check_permission(&team.id, "usr_owner", TeamRole::Admin));
        assert!(store.check_permission(&team.id, "usr_owner", TeamRole::Member));

        // Admin has Admin and Member, but not Owner.
        assert!(!store.check_permission(&team.id, "usr_admin", TeamRole::Owner));
        assert!(store.check_permission(&team.id, "usr_admin", TeamRole::Admin));
        assert!(store.check_permission(&team.id, "usr_admin", TeamRole::Member));

        // Member has only Member.
        assert!(!store.check_permission(&team.id, "usr_member", TeamRole::Owner));
        assert!(!store.check_permission(&team.id, "usr_member", TeamRole::Admin));
        assert!(store.check_permission(&team.id, "usr_member", TeamRole::Member));
    }

    #[test]
    fn remove_member_works() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .unwrap();

        let updated = store.remove_member(&team.id, "usr_bob").unwrap();
        assert_eq!(updated.members.len(), 1);
        assert_eq!(updated.members[0].user_id, "usr_alice");
    }

    #[test]
    fn cannot_remove_owner() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        let err = store.remove_member(&team.id, "usr_alice").unwrap_err();
        assert!(matches!(err, TeamError::CannotRemoveOwner));
    }

    #[test]
    fn remove_nonexistent_member_fails() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        let err = store.remove_member(&team.id, "usr_ghost").unwrap_err();
        assert!(matches!(err, TeamError::UserNotMember { .. }));
    }

    #[test]
    fn update_role_works() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .unwrap();

        let updated = store
            .update_role(&team.id, "usr_bob", TeamRole::Admin)
            .unwrap();

        let bob = updated.members.iter().find(|m| m.user_id == "usr_bob").unwrap();
        assert_eq!(bob.role, TeamRole::Admin);
    }

    #[test]
    fn cannot_change_owner_role() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        let err = store
            .update_role(&team.id, "usr_alice", TeamRole::Member)
            .unwrap_err();
        assert!(matches!(err, TeamError::CannotChangeOwnerRole));
    }

    #[test]
    fn nonexistent_team_returns_none() {
        let store = TeamStore::new();
        assert!(store.get_team("team_nonexistent").is_none());
    }

    #[test]
    fn permission_check_on_nonexistent_team_returns_false() {
        let store = TeamStore::new();
        assert!(!store.check_permission("team_nope", "usr_alice", TeamRole::Member));
    }

    #[test]
    fn permission_check_for_non_member_returns_false() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");
        assert!(!store.check_permission(&team.id, "usr_stranger", TeamRole::Member));
    }

    #[test]
    fn role_display_formats() {
        assert_eq!(TeamRole::Owner.to_string(), "owner");
        assert_eq!(TeamRole::Admin.to_string(), "admin");
        assert_eq!(TeamRole::Member.to_string(), "member");
    }

    #[test]
    fn adding_owner_role_downgrades_to_admin() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice");

        let updated = store
            .add_member(&team.id, "usr_bob", TeamRole::Owner)
            .unwrap();

        let bob = updated.members.iter().find(|m| m.user_id == "usr_bob").unwrap();
        assert_eq!(bob.role, TeamRole::Admin);
    }
}
