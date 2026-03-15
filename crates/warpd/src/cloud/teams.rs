//! Team accounts with role-based access control.
//!
//! Teams allow multiple users to collaborate on shared deployments.
//! Each team has an owner, optional admins, and members. Permission
//! hierarchy: Owner > Admin > Member.
//!
//! Supports two backends:
//! - In-memory (for tests and development without persistence)
//! - libSQL (for production — persists across restarts, edge-replicable)

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
    Storage(String),
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
            Self::Storage(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl std::error::Error for TeamError {}

// ── Backend ─────────────────────────────────────────────────────

/// Team store with pluggable backend (in-memory or libSQL).
#[derive(Clone)]
pub struct TeamStore {
    backend: TeamBackend,
}

#[derive(Clone)]
enum TeamBackend {
    Memory {
        teams: Arc<RwLock<HashMap<String, Team>>>,
    },
    LibSql {
        conn: libsql::Connection,
    },
}

impl TeamStore {
    /// Create an in-memory team store (for tests and dev without persistence).
    pub fn new() -> Self {
        Self {
            backend: TeamBackend::Memory {
                teams: Arc::new(RwLock::new(HashMap::new())),
            },
        }
    }

    /// Create a persistent team store backed by libSQL.
    pub fn with_libsql(conn: libsql::Connection) -> Self {
        Self {
            backend: TeamBackend::LibSql { conn },
        }
    }

    /// Create a new team. The creator becomes the owner.
    pub async fn create_team(&self, name: &str, owner_user_id: &str) -> Team {
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

        match &self.backend {
            TeamBackend::Memory { teams } => {
                let mut store = teams.write().unwrap();
                store.insert(team_id, team.clone());
            }
            TeamBackend::LibSql { conn } => {
                let _ = conn
                    .execute(
                        "INSERT INTO cloud_teams (id, name, owner_user_id, created_at) VALUES (?, ?, ?, ?)",
                        libsql::params![
                            team.id.clone(),
                            team.name.clone(),
                            team.owner_user_id.clone(),
                            now as i64
                        ],
                    )
                    .await;

                let _ = conn
                    .execute(
                        "INSERT INTO cloud_team_members (team_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)",
                        libsql::params![
                            team.id.clone(),
                            owner_user_id.to_string(),
                            "owner".to_string(),
                            now as i64
                        ],
                    )
                    .await;
            }
        }

        team
    }

    /// Get a team by ID.
    pub async fn get_team(&self, team_id: &str) -> Option<Team> {
        match &self.backend {
            TeamBackend::Memory { teams } => {
                let store = teams.read().unwrap();
                store.get(team_id).cloned()
            }
            TeamBackend::LibSql { conn } => {
                let mut rows = conn
                    .query(
                        "SELECT id, name, owner_user_id, created_at FROM cloud_teams WHERE id = ?",
                        libsql::params![team_id.to_string()],
                    )
                    .await
                    .ok()?;

                let row = rows.next().await.ok()??;
                let id: String = row.get(0).ok()?;
                let name: String = row.get(1).ok()?;
                let owner_user_id: String = row.get(2).ok()?;
                let created_at = row.get::<i64>(3).ok()? as u64;

                let members = self.load_members(conn, &id).await;

                Some(Team {
                    id,
                    name,
                    owner_user_id,
                    created_at,
                    members,
                })
            }
        }
    }

    /// List all teams a user belongs to.
    pub async fn list_teams_for_user(&self, user_id: &str) -> Vec<Team> {
        match &self.backend {
            TeamBackend::Memory { teams } => {
                let store = teams.read().unwrap();
                store
                    .values()
                    .filter(|team| team.members.iter().any(|m| m.user_id == user_id))
                    .cloned()
                    .collect()
            }
            TeamBackend::LibSql { conn } => {
                let mut team_rows = match conn
                    .query(
                        "SELECT t.id, t.name, t.owner_user_id, t.created_at \
                         FROM cloud_teams t \
                         JOIN cloud_team_members m ON t.id = m.team_id \
                         WHERE m.user_id = ?",
                        libsql::params![user_id.to_string()],
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(_) => return Vec::new(),
                };

                let mut teams = Vec::new();
                while let Ok(Some(row)) = team_rows.next().await {
                    let id: String = row.get(0).unwrap_or_default();
                    let name: String = row.get(1).unwrap_or_default();
                    let owner_user_id: String = row.get(2).unwrap_or_default();
                    let created_at = row.get::<i64>(3).unwrap_or_default() as u64;

                    let members = self.load_members(conn, &id).await;

                    teams.push(Team {
                        id,
                        name,
                        owner_user_id,
                        created_at,
                        members,
                    });
                }

                teams
            }
        }
    }

    /// Add a member to a team.
    pub async fn add_member(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<Team, TeamError> {
        // Cannot add a second owner.
        let actual_role = if role == TeamRole::Owner {
            TeamRole::Admin
        } else {
            role
        };

        match &self.backend {
            TeamBackend::Memory { teams } => {
                let mut store = teams.write().unwrap();
                let team = store
                    .get_mut(team_id)
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })?;

                if team.members.iter().any(|m| m.user_id == user_id) {
                    return Err(TeamError::UserAlreadyMember {
                        user_id: user_id.to_string(),
                    });
                }

                team.members.push(TeamMember {
                    user_id: user_id.to_string(),
                    role: actual_role,
                    joined_at: epoch_secs(),
                });

                Ok(team.clone())
            }
            TeamBackend::LibSql { conn } => {
                // Verify team exists.
                let team = self
                    .get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })?;

                // Check if user is already a member.
                if team.members.iter().any(|m| m.user_id == user_id) {
                    return Err(TeamError::UserAlreadyMember {
                        user_id: user_id.to_string(),
                    });
                }

                let now = epoch_secs();
                conn.execute(
                    "INSERT INTO cloud_team_members (team_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)",
                    libsql::params![
                        team_id.to_string(),
                        user_id.to_string(),
                        actual_role.to_string(),
                        now as i64
                    ],
                )
                .await
                .map_err(|e| TeamError::Storage(e.to_string()))?;

                // Return updated team.
                self.get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })
            }
        }
    }

    /// Remove a member from a team. The owner cannot be removed.
    pub async fn remove_member(&self, team_id: &str, user_id: &str) -> Result<Team, TeamError> {
        match &self.backend {
            TeamBackend::Memory { teams } => {
                let mut store = teams.write().unwrap();
                let team = store
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
            TeamBackend::LibSql { conn } => {
                let team = self
                    .get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })?;

                if team.owner_user_id == user_id {
                    return Err(TeamError::CannotRemoveOwner);
                }

                if !team.members.iter().any(|m| m.user_id == user_id) {
                    return Err(TeamError::UserNotMember {
                        user_id: user_id.to_string(),
                    });
                }

                conn.execute(
                    "DELETE FROM cloud_team_members WHERE team_id = ? AND user_id = ?",
                    libsql::params![team_id.to_string(), user_id.to_string()],
                )
                .await
                .map_err(|e| TeamError::Storage(e.to_string()))?;

                self.get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })
            }
        }
    }

    /// Update a member's role. Cannot change the owner's role.
    pub async fn update_role(
        &self,
        team_id: &str,
        user_id: &str,
        role: TeamRole,
    ) -> Result<Team, TeamError> {
        // Cannot promote someone to Owner through role update.
        let actual_role = if role == TeamRole::Owner {
            TeamRole::Admin
        } else {
            role
        };

        match &self.backend {
            TeamBackend::Memory { teams } => {
                let mut store = teams.write().unwrap();
                let team = store
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

                member.role = actual_role;
                Ok(team.clone())
            }
            TeamBackend::LibSql { conn } => {
                let team = self
                    .get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })?;

                if team.owner_user_id == user_id {
                    return Err(TeamError::CannotChangeOwnerRole);
                }

                if !team.members.iter().any(|m| m.user_id == user_id) {
                    return Err(TeamError::UserNotMember {
                        user_id: user_id.to_string(),
                    });
                }

                conn.execute(
                    "UPDATE cloud_team_members SET role = ? WHERE team_id = ? AND user_id = ?",
                    libsql::params![
                        actual_role.to_string(),
                        team_id.to_string(),
                        user_id.to_string()
                    ],
                )
                .await
                .map_err(|e| TeamError::Storage(e.to_string()))?;

                self.get_team(team_id)
                    .await
                    .ok_or_else(|| TeamError::TeamNotFound {
                        team_id: team_id.to_string(),
                    })
            }
        }
    }

    /// Check if a user has at least the required role in a team.
    pub async fn check_permission(
        &self,
        team_id: &str,
        user_id: &str,
        required_role: TeamRole,
    ) -> bool {
        match &self.backend {
            TeamBackend::Memory { teams } => {
                let store = teams.read().unwrap();
                let Some(team) = store.get(team_id) else {
                    return false;
                };

                team.members
                    .iter()
                    .find(|m| m.user_id == user_id)
                    .map(|m| m.role.has_privilege(required_role))
                    .unwrap_or(false)
            }
            TeamBackend::LibSql { conn } => {
                let mut rows = match conn
                    .query(
                        "SELECT role FROM cloud_team_members WHERE team_id = ? AND user_id = ?",
                        libsql::params![team_id.to_string(), user_id.to_string()],
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(_) => return false,
                };

                let row = match rows.next().await {
                    Ok(Some(r)) => r,
                    _ => return false,
                };

                let role_str: String = match row.get(0) {
                    Ok(r) => r,
                    Err(_) => return false,
                };

                let role = match role_str.as_str() {
                    "owner" => TeamRole::Owner,
                    "admin" => TeamRole::Admin,
                    _ => TeamRole::Member,
                };

                role.has_privilege(required_role)
            }
        }
    }

    // ── libSQL helper ───────────────────────────────────────────

    /// Load all members for a team from libSQL.
    async fn load_members(&self, conn: &libsql::Connection, team_id: &str) -> Vec<TeamMember> {
        let mut rows = match conn
            .query(
                "SELECT user_id, role, joined_at FROM cloud_team_members WHERE team_id = ?",
                libsql::params![team_id.to_string()],
            )
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut members = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let user_id: String = row.get(0).unwrap_or_default();
            let role_str: String = row.get(1).unwrap_or_default();
            let joined_at = row.get::<i64>(2).unwrap_or_default() as u64;

            let role = match role_str.as_str() {
                "owner" => TeamRole::Owner,
                "admin" => TeamRole::Admin,
                _ => TeamRole::Member,
            };

            members.push(TeamMember {
                user_id,
                role,
                joined_at,
            });
        }

        members
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

    // ── In-memory backend tests (existing) ──────────────────────

    #[tokio::test]
    async fn create_team_sets_owner() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        assert!(team.id.starts_with("team_"));
        assert_eq!(team.name, "Acme");
        assert_eq!(team.owner_user_id, "usr_alice");
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].user_id, "usr_alice");
        assert_eq!(team.members[0].role, TeamRole::Owner);
    }

    #[tokio::test]
    async fn add_member_and_list() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        let updated = store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .await
            .unwrap();
        assert_eq!(updated.members.len(), 2);

        let bob = updated
            .members
            .iter()
            .find(|m| m.user_id == "usr_bob")
            .unwrap();
        assert_eq!(bob.role, TeamRole::Member);

        // List teams for bob.
        let bob_teams = store.list_teams_for_user("usr_bob").await;
        assert_eq!(bob_teams.len(), 1);
        assert_eq!(bob_teams[0].id, team.id);
    }

    #[tokio::test]
    async fn add_duplicate_member_fails() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .await
            .unwrap();

        let err = store
            .add_member(&team.id, "usr_bob", TeamRole::Admin)
            .await
            .unwrap_err();
        assert!(matches!(err, TeamError::UserAlreadyMember { .. }));
    }

    #[tokio::test]
    async fn permission_hierarchy() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_owner").await;

        store
            .add_member(&team.id, "usr_admin", TeamRole::Admin)
            .await
            .unwrap();
        store
            .add_member(&team.id, "usr_member", TeamRole::Member)
            .await
            .unwrap();

        // Owner has all permissions.
        assert!(
            store
                .check_permission(&team.id, "usr_owner", TeamRole::Owner)
                .await
        );
        assert!(
            store
                .check_permission(&team.id, "usr_owner", TeamRole::Admin)
                .await
        );
        assert!(
            store
                .check_permission(&team.id, "usr_owner", TeamRole::Member)
                .await
        );

        // Admin has Admin and Member, but not Owner.
        assert!(
            !store
                .check_permission(&team.id, "usr_admin", TeamRole::Owner)
                .await
        );
        assert!(
            store
                .check_permission(&team.id, "usr_admin", TeamRole::Admin)
                .await
        );
        assert!(
            store
                .check_permission(&team.id, "usr_admin", TeamRole::Member)
                .await
        );

        // Member has only Member.
        assert!(
            !store
                .check_permission(&team.id, "usr_member", TeamRole::Owner)
                .await
        );
        assert!(
            !store
                .check_permission(&team.id, "usr_member", TeamRole::Admin)
                .await
        );
        assert!(
            store
                .check_permission(&team.id, "usr_member", TeamRole::Member)
                .await
        );
    }

    #[tokio::test]
    async fn remove_member_works() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .await
            .unwrap();

        let updated = store.remove_member(&team.id, "usr_bob").await.unwrap();
        assert_eq!(updated.members.len(), 1);
        assert_eq!(updated.members[0].user_id, "usr_alice");
    }

    #[tokio::test]
    async fn cannot_remove_owner() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        let err = store
            .remove_member(&team.id, "usr_alice")
            .await
            .unwrap_err();
        assert!(matches!(err, TeamError::CannotRemoveOwner));
    }

    #[tokio::test]
    async fn remove_nonexistent_member_fails() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        let err = store
            .remove_member(&team.id, "usr_ghost")
            .await
            .unwrap_err();
        assert!(matches!(err, TeamError::UserNotMember { .. }));
    }

    #[tokio::test]
    async fn update_role_works() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .await
            .unwrap();

        let updated = store
            .update_role(&team.id, "usr_bob", TeamRole::Admin)
            .await
            .unwrap();

        let bob = updated
            .members
            .iter()
            .find(|m| m.user_id == "usr_bob")
            .unwrap();
        assert_eq!(bob.role, TeamRole::Admin);
    }

    #[tokio::test]
    async fn cannot_change_owner_role() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        let err = store
            .update_role(&team.id, "usr_alice", TeamRole::Member)
            .await
            .unwrap_err();
        assert!(matches!(err, TeamError::CannotChangeOwnerRole));
    }

    #[tokio::test]
    async fn nonexistent_team_returns_none() {
        let store = TeamStore::new();
        assert!(store.get_team("team_nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn permission_check_on_nonexistent_team_returns_false() {
        let store = TeamStore::new();
        assert!(
            !store
                .check_permission("team_nope", "usr_alice", TeamRole::Member)
                .await
        );
    }

    #[tokio::test]
    async fn permission_check_for_non_member_returns_false() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;
        assert!(
            !store
                .check_permission(&team.id, "usr_stranger", TeamRole::Member)
                .await
        );
    }

    #[test]
    fn role_display_formats() {
        assert_eq!(TeamRole::Owner.to_string(), "owner");
        assert_eq!(TeamRole::Admin.to_string(), "admin");
        assert_eq!(TeamRole::Member.to_string(), "member");
    }

    #[tokio::test]
    async fn adding_owner_role_downgrades_to_admin() {
        let store = TeamStore::new();
        let team = store.create_team("Acme", "usr_alice").await;

        let updated = store
            .add_member(&team.id, "usr_bob", TeamRole::Owner)
            .await
            .unwrap();

        let bob = updated
            .members
            .iter()
            .find(|m| m.user_id == "usr_bob")
            .unwrap();
        assert_eq!(bob.role, TeamRole::Admin);
    }

    // ── libSQL backend tests ────────────────────────────────────

    #[tokio::test]
    async fn libsql_create_and_get_team() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = TeamStore::with_libsql(conn);
        let team = store.create_team("Acme", "usr_alice").await;

        assert!(team.id.starts_with("team_"));
        assert_eq!(team.name, "Acme");
        assert_eq!(team.owner_user_id, "usr_alice");
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].role, TeamRole::Owner);

        // Read it back.
        let fetched = store.get_team(&team.id).await.unwrap();
        assert_eq!(fetched.name, "Acme");
        assert_eq!(fetched.owner_user_id, "usr_alice");
        assert_eq!(fetched.members.len(), 1);
    }

    #[tokio::test]
    async fn libsql_add_member_and_list() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let store = TeamStore::with_libsql(conn);
        let team = store.create_team("Acme", "usr_alice").await;

        let updated = store
            .add_member(&team.id, "usr_bob", TeamRole::Member)
            .await
            .unwrap();
        assert_eq!(updated.members.len(), 2);

        let bob = updated
            .members
            .iter()
            .find(|m| m.user_id == "usr_bob")
            .unwrap();
        assert_eq!(bob.role, TeamRole::Member);

        // List teams for bob.
        let bob_teams = store.list_teams_for_user("usr_bob").await;
        assert_eq!(bob_teams.len(), 1);
        assert_eq!(bob_teams[0].id, team.id);
    }
}
