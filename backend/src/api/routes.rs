use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::api::AppState;
use crate::api::middleware::require_auth;
use crate::auth::handlers::{login_handler, logout_handler, me_handler, refresh_handler};
use crate::menu::handlers::{
    create_menu_handler, delete_menu_handler, get_all_menus_handler, get_menu_handler,
    get_menu_tree_handler, get_menus_by_parent_handler, get_user_menu_handler, update_menu_handler,
};
use crate::org::handlers::{
    create_org_handler, delete_org_handler, get_all_orgs_handler, get_org_handler,
    get_org_tree_handler, get_orgs_by_parent_handler, update_org_handler,
};
use crate::role::handlers::{
    assign_role_to_user_handler, create_role_handler, delete_role_handler, get_all_roles_handler,
    get_role_handler, get_role_users_handler, get_roles_page_handler, get_user_roles_handler,
    remove_role_from_user_handler, update_role_handler,
};
use crate::user::handlers::{
    create_user_handler, delete_user_handler, get_all_users_handler, get_user_handler,
    get_users_page_handler, update_user_handler,
};

pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login_handler))
        .route("/auth/logout", post(logout_handler))
        .route("/auth/me", get(me_handler))
        .route("/auth/refresh", post(refresh_handler))
}

pub fn user_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/users/page", get(get_users_page_handler))
        .route(
            "/users/",
            post(create_user_handler).get(get_all_users_handler),
        )
        .route(
            "/users/{id}",
            get(get_user_handler)
                .put(update_user_handler)
                .delete(delete_user_handler),
        )
        .route(
            "/users/{user_id}/roles/{role_id}",
            post(assign_role_to_user_handler).delete(remove_role_from_user_handler),
        )
        .route("/users/{user_id}/roles", get(get_user_roles_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn role_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/roles/page", get(get_roles_page_handler))
        .route(
            "/roles/",
            post(create_role_handler).get(get_all_roles_handler),
        )
        .route(
            "/roles/{id}",
            get(get_role_handler)
                .put(update_role_handler)
                .delete(delete_role_handler),
        )
        .route("/roles/{id}/users", get(get_role_users_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn menu_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/menus/",
            post(create_menu_handler).get(get_all_menus_handler),
        )
        .route(
            "/menus/{id}",
            get(get_menu_handler)
                .put(update_menu_handler)
                .delete(delete_menu_handler),
        )
        .route("/menus/tree", get(get_menu_tree_handler))
        .route("/menus/parent", get(get_menus_by_parent_handler))
        .route("/menus/user-menu", get(get_user_menu_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn org_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/orgs/", post(create_org_handler).get(get_all_orgs_handler))
        .route(
            "/orgs/{id}",
            get(get_org_handler)
                .put(update_org_handler)
                .delete(delete_org_handler),
        )
        .route("/orgs/tree", get(get_org_tree_handler))
        .route("/orgs/parent", get(get_orgs_by_parent_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .merge(auth_routes())
        .merge(user_routes(state.clone()))
        .merge(role_routes(state.clone()))
        .merge(menu_routes(state.clone()))
        .merge(org_routes(state.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state)
}
