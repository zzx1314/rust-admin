# AGENTS.md — x-rust

## Project Overview
Rust web API built with **axum 0.8** + **sea-orm 1.1 (SQLite)** + **tokio**. Edition 2024.
Layered architecture with trait-based DI, inspired by Spring patterns.

## Build / Run / Test Commands

```bash
cargo build                  # Build
cargo build --release        # Release build
cargo run                    # Run (DATABASE_URL defaults to sqlite:data/users.db)
RUST_LOG=debug cargo run     # Run with debug logging
cargo check                  # Check without building
cargo test                   # Run all tests (unit tests only, see note below)
cargo test -- <test_name>    # Run a single test
cargo test --test <file>     # Run a specific test file
cargo test --test auth_service_tests  # Run auth service tests

# Note: Tests use UUID-based database files (e.g., user_repo_{uuid}.db) created automatically.
cargo clippy -- -D warnings  # Lint (zero warnings required)
cargo fmt && cargo fmt -- --check  # Format
```

## Architecture

```
src/
├── main.rs                     # Entry point: tracing + app.run()
├── app.rs                      # Application assembler: DB pool, DI wiring
├── lib.rs                      # Library crate root (pub mod exports)
│
├── config/                     # Configuration layer
│   └── mod.rs                 # AppConfig (Redis URL, JWT secret, etc.)
│
├── user/                       # User module (按模块划分)
│   ├── entity.rs              # SeaORM User entity
│   ├── domain.rs              # User type alias + CRUD DTO + UserVO
│   ├── user.rs               # Placeholder for future extensions (currently unused/empty)
│   ├── repository.rs          # UserRepository trait + SeaOrmUserRepository impl
│   ├── service.rs             # UserService (injects Arc<dyn UserRepository>)
│   └── handlers.rs            # User CRUD + pagination HTTP handlers
│
├── role/                       # Role module
│   ├── entity.rs              # SeaORM Role entity
│   ├── user_role.rs           # User-Role relation entity
│   ├── sys_role_menu.rs       # Role-Menu relation entity
│   ├── domain.rs              # Role type alias + CRUD DTO
│   ├── repository.rs          # RoleRepository trait + impl
│   ├── service.rs             # RoleService
│   └── handlers.rs            # Role HTTP handlers
│
├── menu/                       # Menu module
│   ├── entity.rs              # SeaORM Menu entity
│   ├── domain.rs              # Menu type alias + CRUD DTO + MenuVo + MenuTree
│   ├── repository.rs          # MenuRepository trait + impl
│   ├── service.rs             # MenuService
│   └── handlers.rs            # Menu CRUD + tree HTTP handlers
│
├── org/                        # Org module
│   ├── entity.rs              # SeaORM Org entity
│   ├── domain.rs              # Org type alias + CRUD DTO + OrgTreeDto
│   ├── repository.rs          # OrgRepository trait + impl
│   ├── service.rs             # OrgService
│   └── handlers.rs            # Org CRUD + tree HTTP handlers
│
├── auth/                       # Auth module
│   ├── repository.rs          # TokenStore trait + RedisTokenStore impl
│   ├── service.rs             # AuthService (JWT + TokenStore + UserRepository)
│   └── handlers.rs            # Login/logout/validate HTTP handlers
│
├── common/                     # Common/shared code
│   ├── base.rs              # BaseRepository, RepoExt traits
│   ├── sqlite/               # SQLite query helpers
│   │   ├── mod.rs
│   │   └── query_builder.rs
│   ├── error.rs              # AppError enum, ApiResponse<T>, IntoResponse impl
│   ├── traits.rs             # Repository trait definitions
│   ├── util.rs               # md5_encrypt, encrypt_password helpers
│   ├── pagination.rs          # PageRequest, PageResponse<T>
│   └── value_objects.rs       # Email value object (encapsulates validation)
│
└── api/                        # HTTP interface layer
    ├── mod.rs                 # AppState (IoC container, must be Clone)
    ├── handlers/mod.rs        # Handler utilities (currently unused/empty)
    ├── routes.rs              # Router assembly with .merge() per resource
    └── middleware.rs          # require_auth middleware

tests/                          # Integration tests (organized by module)
├── user/
│   ├── user_service_tests.rs      # UserService unit tests + FakeUserRepository
│   ├── user_repository_tests.rs   # UserRepository integration tests
│   └── user_api_tests.rs          # User HTTP API tests
├── role/
│   ├── role_service_tests.rs      # RoleService unit tests + FakeRoleRepository
│   ├── role_repository_tests.rs   # RoleRepository integration tests
│   └── role_api_tests.rs          # Role HTTP API tests
├── menu/
│   ├── menu_service_tests.rs      # MenuService unit tests + FakeMenuRepository
│   └── menu_repository_tests.rs   # MenuRepository integration tests
├── org/
│   ├── org_service_tests.rs       # OrgService unit tests + FakeOrgRepository
│   ├── org_repository_tests.rs    # OrgRepository integration tests
│   └── org_api_tests.rs           # Org HTTP API tests
├── auth/
│   └── auth_service_tests.rs      # AuthService unit tests + FakeTokenStore
└── common/
    ├── email_tests.rs             # Email value object tests
    ├── error_tests.rs             # AppError tests
    └── util_tests.rs              # Utility function tests
```

### Test Configuration
Tests are organized by module under `tests/<module>/`. Each test file is explicitly declared in `Cargo.toml` via `[[test]]` entries. Run with `cargo test --test <name>` (name from `[[test]]` block, not file path).

### Code Conventions

### Domain Layer Patterns
- Use **type alias** to entity models: `pub type User = UserModel;`
- Keep CRUD request/response DTOs in domain layer (`CreateXxxRequest`, `UpdateXxxRequest`, `XxxVO`)
- Keep view-specific structs in domain layer (`MenuTree`, `OrgTreeDto`, `build_*_tree` functions)
- Import entity models: `use crate::entity::user::Model as UserModel;`

### Models
- Models derive: `Debug`, `Serialize`, `Deserialize`, `Clone`, `FromRow`
- Request structs derive: `Debug`, `Deserialize`
- Value objects: `Debug`, `Clone` + constructor that returns `Result<Self, String>`
- `AppState` must derive `Clone` (required by axum Router)
- Pagination: use `PageRequest` (query params `current`/`size`, defaults 1/10) and `PageResponse<T>` (records/total/current/size)

### SQL Conventions (sea-orm)
- Use Entity Trait for table operations (e.g., `UserEntity::find_by_id(id).one(&conn).await`)
- Use ActiveModel for inserts/updates with `ActiveValue`
- Use `query_as` for custom queries returning entity types
- Positional params in raw queries: `?1`, `?2`, `?3`

#### SQL Column Reuse with `macro_rules!` + `concat!`
When the same column list appears in 3+ places, extract it into a macro. This avoids duplication and ensures consistency. Use `macro_rules!` + `concat!` (not `const` or `format!`):
- `macro_rules!` — defines the reusable SQL fragment
- `concat!` — compiles the fragment into a string literal at compile time

```rust
// ✅ Correct: macro_rules! + concat! produces a compile-time string literal
macro_rules! user_select {
    () => {
        concat!(
            "SELECT id, username, phone, email, real_name, password, org_id, lock_time, ",
            "last_login_time, try_count, lock_flag, create_time, update_time, ",
            "is_deleted, remarks, pass_update_time, card, is_show, enable, first_login, sex ",
            "FROM users"
        )
    };
}

// Usage with sea-orm query_as:
query_as::<_, User>(user_select!())

// ❌ Wrong: const fn — Rust doesn't expand fn calls inside string literals
// ❌ Wrong: format! — returns String, not a string literal, breaking macro
// ❌ Wrong: concat! in raw string — not a macro call, treated as literal text
```

Common patterns:
```rust
// For SELECT ... FROM <table>
macro_rules! <entity>_select {
    () => { concat!(...columns..., " FROM <table>") };
}

// For INSERT ... RETURNING
macro_rules! <entity>_returning {
    () => { concat!(...columns...) };
}

// Usage: concat!(<entity>_select!(), " WHERE id = ?1")
```

**Actual patterns in codebase** (different from above examples):
- `soft_delete_filter!` — generates column.eq(0) condition for soft delete
- `impl_repo_conn!` — generates BaseRepository impl for repository structs
- `bind_filters!` — binds filter values to query (2 or 4 args for pagination)

These macros focus on query builder utilities, not SQL column reuse. The column reuse pattern is documented but not currently used.

### Repository Coding Conventions (SeaORM)

#### 1. create method

**MUST** use domain layer's `to_active_model` method. DO NOT manually build ActiveModel in repository:

```rust
// ✅ Correct: reuse domain's to_active_model
fn create(&self, req: &CreateUserRequest, id: &str) -> DynFuture<SeaOrmResult<User>> {
    let req = req.clone();
    let id_str = id.to_string();
    self.with_conn(move |conn| {
        Box::pin(async move {
            let now = chrono::Utc::now();
            let active_model = req.to_active_model(&id_str, now);

            UserEntity::insert(active_model).exec(&*conn).await?;

            let user = UserEntity::find_by_id(&id_str).one(&*conn).await?;
            Ok(user.unwrap())
        })
    })
}

// ❌ Wrong: manually build ActiveModel (deprecated pattern)
fn create(&self, req: &CreateUserRequest, id: &str) -> DynFuture<SeaOrmResult<User>> {
    let id_str = id.to_string();
    let name = req.name.clone();
    // ... manually set each field
    self.with_conn(move |conn| {
        Box::pin(async move {
            let active_model = UserActiveModel {
                id: ActiveValue::set(id_str.clone()),
                name: ActiveValue::set(name),
                // ... other fields
            };
            // ...
        })
    })
}
```

**Domain `to_active_model` MUST**:
- Accept `id: &str` and `now: DateTime<Utc>` parameters
- Set `is_deleted: ActiveValue::set(0)`
- Set `create_time` and `update_time` to current time

#### 2. find_all_with_page / find_tree_with_filter methods

Use array + `filter_map` + loop pattern for building query conditions:

```rust
fn find_all_with_page(&self, req: &UserPageQuery) -> DynFuture<SeaOrmResult<(Vec<UserVO>, i64)>> {
    let req = req.clone();
    self.with_conn(move |conn| {
        Box::pin(async move {
            let base_query = UserEntity::find().filter(UserColumn::IsDeleted.eq(0));

            let mut cond = make_condition();
            let conditions: Vec<_> = [
                req.username.as_ref().map(|v| UserColumn::Username.contains(v)),
                req.real_name.as_ref().map(|v| UserColumn::RealName.contains(v)),
                req.phone.as_ref().map(|v| UserColumn::Phone.contains(v)),
                // ... other optional query conditions
            ]
            .into_iter()
            .filter_map(|c| c)
            .collect();
            for c in conditions {
                cond = cond.add(c);
            }

            let total = base_query.clone().filter(cond.clone()).count(&*conn).await?;

            let offset = (req.page() - 1) * req.size();
            let records = base_query
                .filter(cond)
                .order_by(UserColumn::CreateTime, order_desc())
                .offset(Some(offset as u64))
                .limit(req.size() as u64)
                .all(&*conn)
                .await?
                .into_iter()
                .map(UserVO::from)
                .collect();

            Ok((records, total as i64))
        })
    })
}
```

**Note**: If query conditions may all be empty, check upfront and skip filtering:

```rust
fn find_tree_with_filter(&self, query: &OrgTreeQuery) -> DynFuture<SeaOrmResult<Vec<Org>>> {
    let query = query.clone();
    self.with_conn(move |conn| {
        Box::pin(async move {
            let base_query = OrgEntity::find().filter(OrgColumn::IsDeleted.eq(0));
            let has_filter = query.name.is_some() || query.r#type.is_some();

            if !has_filter {
                // No filter conditions, return directly
                return Ok(base_query
                    .order_by(OrgColumn::Sort, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect());
            }

            // Has filter conditions
            let mut cond = make_condition();
            if let Some(ref name) = query.name {
                cond = cond.add(OrgColumn::Name.contains(name));
            }
            // ...

            Ok(base_query
                .filter(cond)
                .order_by(OrgColumn::Sort, order_asc())
                .all(&*conn)
                .await?
                .into_iter()
                .collect())
        })
    })
}
```

#### 3. update method

Check existence first, then update, finally query to return:

```rust
fn update(&self, id: &str, req: &UpdateUserRequest) -> DynFuture<SeaOrmOptResult<User>> {
    let req = req.clone();
    let id_str = id.to_string();
    self.with_conn(move |conn| {
        Box::pin(async move {
            let exists = UserEntity::find()
                .filter(UserColumn::Id.eq(&id_str))
                .filter(UserColumn::IsDeleted.eq(0))
                .one(&*conn)
                .await?;

            if exists.is_none() {
                return Ok(None);
            }

            let active_model = req.to_active_model(&id_str);
            UserEntity::update(active_model)
                .filter(UserColumn::Id.eq(&id_str))
                .filter(UserColumn::IsDeleted.eq(0))
                .exec(&*conn)
                .await?;

            let user = UserEntity::find()
                .filter(UserColumn::Id.eq(&id_str))
                .filter(UserColumn::IsDeleted.eq(0))
                .one(&*conn)
                .await?;
            Ok(user)
        })
    })
}
```

#### 4. delete method (soft delete)

```rust
fn delete(&self, id: &str) -> DynFuture<SeaOrmResult<bool>> {
    let id = id.to_string();
    self.with_conn(move |conn| {
        Box::pin(async move {
            let entity = Entity::find()
                .filter(Column::Id.eq(&id))
                .filter(Column::IsDeleted.eq(0))
                .one(&*conn)
                .await?;

            if let Some(mut entity) = entity {
                entity.is_deleted = 1;
                entity.update_time = Some(chrono::Utc::now());
                let mut active_model: ActiveModel = entity.into();
                active_model.is_deleted = ActiveValue::Set(1);
                Entity::update(active_model).exec(&*conn).await?;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    })
}
```

### Logging
- `tracing::info!` for operational events
- `tracing::error!` for failures
- `tracing::debug!` for verbose details
- Env filter: `RUST_LOG` defaults to `x_rust=debug,tower_http=debug,axum=debug`

### HTTP Response Patterns
- Success: `Ok(Json(entity))`
- No content: `Ok((StatusCode::NO_CONTENT, ()))`
- Errors via `AppError::IntoResponse`

### No Pre-existing Config
- No `rustfmt.toml` — use default `cargo fmt`
- No `.clippy.toml` — default clippy, must pass `-D warnings`
- No comments unless absolutely necessary (code must be self-documenting)

---

### Core Principles: Code Structure Integrity and Context Awareness

You are a senior programming expert with extreme sensitivity to code structure. When performing any code modifications, refactoring, or generation tasks, you must strictly adhere to the following protocol:

#### 1. Prioritize Global Context

- **Prohibit blind local modifications**: Before modifying any function or code block, you must first read and understand the complete parent function or complete class that contains the target.
- **Dependency analysis**: Check the nesting level of the target code block, confirming its dependencies on external variables and internal logic flow.

#### 2. Bracket and Syntax Structure Protection

- **Mandatory bracket matching check**: When generating or modifying code, you must check all `(`, `)`, `{`, `}`, `[`, `]` for proper pairing.
- **Defensive modifications**: If you need to insert code into a complex nested structure, you must ensure you do not break existing closing logic.
- **Self-correction**: If you are uncertain about the scope of a bracket, **you must first read the complete code within that scope**—never guess.

#### 3. Modification Execution Workflow

1. **Read**: Read the affected complete function/module.
2. **Plan**: Construct an abstract syntax tree (AST) in your mind to locate the modification point.
3. **Modify**: Apply changes, ensuring indentation and bracket levels match the original code exactly.
4. **Verify**: Before outputting the final code, ask yourself: "Did I accidentally delete a closing bracket? Did I break the function's scope?"

#### 4. Error Prevention

- **Never** "patch" using only local code fragments without reading the complete context.
- If you find mismatched brackets in the original code, prioritize pointing out the issue rather than forcibly modifying on top of it.
