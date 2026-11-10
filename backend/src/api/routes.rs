use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware::from_fn_with_state,
    routing::{get, post, put},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::api::AppState;
use crate::api::middleware::{audit_log_middleware, require_auth};
use crate::business::dog::command::handlers as dog_command_handlers;
use crate::business::dog::config::handlers as dog_config_handlers;
use crate::business::dog::map::handlers as dog_map_handlers;
use crate::business::dog::media::handlers as dog_media_handlers;
use crate::business::dog::model::handlers as dog_model_handlers;
use crate::business::dog::services::handlers as dog_services_handlers;
use crate::business::dog::statistics::handlers as dog_statistics_handlers;
use crate::system::auth::handlers::{
    check_token_handler, login_handler, logout_handler, me_handler, refresh_handler,
};
use crate::system::sys_auth::handlers::{get_menu_data_handler, set_menu_auth_handler};
use crate::system::sys_dict::handlers::{
    create_dict_handler, delete_dict_handler, get_all_dicts_handler, get_dict_handler,
    get_dicts_page_handler, update_dict_handler,
};
use crate::system::sys_dict_item::handlers::{
    create_dict_item_handler, delete_dict_item_handler, get_all_dict_items_handler,
    get_dict_item_handler, get_dict_items_by_dict_id_handler, get_dict_items_by_type_handler,
    get_dict_items_page_handler, get_safe_policy_handler, update_dict_item_handler,
    update_safe_policy_handler,
};
use crate::system::sys_log::handlers::{
    create_log_handler, delete_log_handler, get_all_logs_handler, get_log_handler,
    get_logs_page_handler, update_log_handler,
};
use crate::system::sys_menu::handlers::{
    create_menu_handler, delete_menu_handler, get_all_menus_handler, get_menu_handler,
    get_menu_tree_handler, get_menus_by_parent_handler, get_user_menu_handler, update_menu_handler,
};
use crate::system::sys_org::handlers::{
    create_org_handler, delete_org_handler, get_all_orgs_handler, get_org_handler,
    get_org_tree_handler, get_orgs_by_parent_handler, remove_orgs_by_ids_handler,
    update_org_handler,
};
use crate::system::sys_role::handlers::{
    assign_role_to_user_handler, create_role_handler, delete_role_handler, get_all_roles_handler,
    get_role_handler, get_role_users_handler, get_roles_nolog_handler, get_roles_page_handler,
    get_user_roles_handler, remove_role_from_user_handler, update_role_handler,
};
use crate::system::sys_user::handlers::{
    create_user_handler, delete_user_handler, edit_password_handler, get_all_users_handler,
    get_user_handler, get_users_by_role_handler, get_users_page_handler,
    reset_user_password_handler, toggle_user_enable_handler, update_user_handler,
};

pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/token", post(login_handler))
        .route("/token/logout", get(logout_handler))
        .route("/token/refresh/{refresh_token}", get(refresh_handler))
        .route("/token/check_token", get(check_token_handler))
}

pub fn sys_user_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysUser/info", get(me_handler))
        .route("/sysUser/edit", put(edit_password_handler))
        .route(
            "/sysUser/getUserByRoleIdNoPage",
            get(get_users_by_role_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn user_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysUser/getPage", get(get_users_page_handler))
        .route(
            "/sysUser",
            post(create_user_handler).get(get_all_users_handler),
        )
        .route(
            "/sysUser/{id}",
            get(get_user_handler)
                .put(update_user_handler)
                .delete(delete_user_handler),
        )
        .route("/sysUser/resetPwd", put(reset_user_password_handler))
        .route("/sysUser/enable", put(toggle_user_enable_handler))
        .route(
            "/sysUser/{user_id}/roles/{role_id}",
            post(assign_role_to_user_handler).delete(remove_role_from_user_handler),
        )
        .route("/sysUser/{user_id}/roles", get(get_user_roles_handler))
        .route(
            "/sysUser/getUserByRoleId/{role_id}",
            get(get_users_by_role_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn role_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysRole/getPage", get(get_roles_page_handler))
        .route(
            "/sysRole",
            post(create_role_handler).get(get_all_roles_handler),
        )
        .route(
            "/sysRole/{id}",
            get(get_role_handler)
                .put(update_role_handler)
                .delete(delete_role_handler),
        )
        .route("/sysRole/getAll", get(get_all_roles_handler))
        .route("/sysRole/listNoLog", get(get_roles_nolog_handler))
        .route("/sysRole/{id}/users", get(get_role_users_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn menu_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysMenu/getAll", get(get_all_menus_handler))
        .route(
            "/sysMenu",
            post(create_menu_handler).get(get_all_menus_handler),
        )
        .route(
            "/sysMenu/{id}",
            get(get_menu_handler)
                .put(update_menu_handler)
                .delete(delete_menu_handler),
        )
        .route("/sysMenu/tree", get(get_menu_tree_handler))
        .route("/sysMenu/parent", get(get_menus_by_parent_handler))
        .route("/sysMenu/user-menu", get(get_user_menu_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn org_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysOrg/allList", get(get_all_orgs_handler))
        .route(
            "/sysOrg",
            post(create_org_handler).get(get_all_orgs_handler),
        )
        .route(
            "/sysOrg/{id}",
            get(get_org_handler)
                .put(update_org_handler)
                .delete(delete_org_handler),
        )
        .route("/sysOrg/removeByIds", post(remove_orgs_by_ids_handler))
        .route("/sysOrg/tree", get(get_org_tree_handler))
        .route("/sysOrg/parent", get(get_orgs_by_parent_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn sys_auth_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/sysAuth/getMenuData/{role_code}",
            get(get_menu_data_handler),
        )
        .route("/sysAuth/setMenuAuth", post(set_menu_auth_handler))
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn sys_dict_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysDict/getPage", get(get_dicts_page_handler))
        .route(
            "/sysDict",
            post(create_dict_handler).get(get_all_dicts_handler),
        )
        .route(
            "/sysDict/{id}",
            get(get_dict_handler)
                .put(update_dict_handler)
                .delete(delete_dict_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn sys_dict_item_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysDictItem/getPage", get(get_dict_items_page_handler))
        .route(
            "/sysDictItem",
            post(create_dict_item_handler).get(get_all_dict_items_handler),
        )
        .route(
            "/sysDictItem/{id}",
            get(get_dict_item_handler)
                .put(update_dict_item_handler)
                .delete(delete_dict_item_handler),
        )
        .route(
            "/sysDictItem/getByDictId/{dict_id}",
            get(get_dict_items_by_dict_id_handler),
        )
        .route(
            "/sysDictItem/getByType",
            get(get_dict_items_by_type_handler),
        )
        .route("/sysDict/getSafePolicy", get(get_safe_policy_handler))
        .route(
            "/sysDictItem/updateSafePolicy",
            put(update_safe_policy_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn sys_log_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/sysLog/getPage", get(get_logs_page_handler))
        .route(
            "/sysLog",
            post(create_log_handler).get(get_all_logs_handler),
        )
        .route(
            "/sysLog/{id}",
            get(get_log_handler)
                .put(update_log_handler)
                .delete(delete_log_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_map_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/dogMap/getPage", get(dog_map_handlers::list_maps_handler))
        .route(
            "/dogMap/getAll",
            get(dog_map_handlers::list_all_maps_handler),
        )
        .route("/dogMap", post(dog_map_handlers::create_map_handler))
        .route(
            "/dogMap/upload",
            post(dog_map_handlers::upload_map_handler)
                .layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/dogMap/{id}/image",
            get(dog_map_handlers::get_map_image_handler),
        )
        .route(
            "/dogMap/{id}/publish",
            post(dog_map_handlers::publish_map_handler),
        )
        .route(
            "/dogMap/{id}/newVersion",
            post(dog_map_handlers::new_map_version_handler),
        )
        .route(
            "/dogMap/{id}",
            get(dog_map_handlers::get_map_handler)
                .put(dog_map_handlers::update_map_handler)
                .delete(dog_map_handlers::delete_map_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_media_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogMedia/getPage",
            get(dog_media_handlers::list_media_handler),
        )
        .route(
            "/dogMedia/getAll",
            get(dog_media_handlers::list_all_media_handler),
        )
        .route("/dogMedia", post(dog_media_handlers::create_media_handler))
        .route(
            "/dogMedia/{id}",
            get(dog_media_handlers::get_media_handler)
                .put(dog_media_handlers::update_media_handler)
                .delete(dog_media_handlers::delete_media_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_model_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogModel/getPage",
            get(dog_model_handlers::list_models_handler),
        )
        .route(
            "/dogModel/getAll",
            get(dog_model_handlers::list_all_models_handler),
        )
        .route("/dogModel", post(dog_model_handlers::create_model_handler))
        .route(
            "/dogModel/{id}",
            get(dog_model_handlers::get_model_handler)
                .put(dog_model_handlers::update_model_handler)
                .delete(dog_model_handlers::delete_model_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_config_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogConfig/getPage",
            get(dog_config_handlers::list_configs_handler),
        )
        .route(
            "/dogConfig/getAll",
            get(dog_config_handlers::list_all_configs_handler),
        )
        .route(
            "/dogConfig",
            post(dog_config_handlers::create_config_handler),
        )
        .route(
            "/dogConfig/{id}",
            get(dog_config_handlers::get_config_handler)
                .put(dog_config_handlers::update_config_handler)
                .delete(dog_config_handlers::delete_config_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_command_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogCommand/getPage",
            get(dog_command_handlers::list_commands_handler),
        )
        .route(
            "/dogCommand/getAll",
            get(dog_command_handlers::list_all_commands_handler),
        )
        .route(
            "/dogCommand",
            post(dog_command_handlers::create_command_handler),
        )
        .route(
            "/dogCommand/{id}",
            get(dog_command_handlers::get_command_handler)
                .put(dog_command_handlers::update_command_handler)
                .delete(dog_command_handlers::delete_command_handler),
        )
        .route(
            "/dogCommand/status/{id}",
            put(dog_command_handlers::update_command_status_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_statistics_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogStatistics/all",
            get(dog_statistics_handlers::statistics_all_handler),
        )
        .route(
            "/dogStatistics/allType",
            get(dog_statistics_handlers::statistics_all_type_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

pub fn dog_services_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/dogServices/getAll",
            get(dog_services_handlers::list_all_services_handler),
        )
        .route(
            "/dogServices",
            post(dog_services_handlers::create_services_handler),
        )
        .route(
            "/dogServices/{id}",
            get(dog_services_handlers::get_services_handler)
                .put(dog_services_handlers::update_services_handler)
                .delete(dog_services_handlers::delete_services_handler),
        )
        .layer(from_fn_with_state(state.clone(), require_auth))
}

/// Device facing gateway routes: token issuing and registration carry a device
/// token (not a user JWT), and the WebSocket path authenticates via its URL.

/// Admin facing gateway routes (user JWT required).

pub fn create_router(state: AppState) -> Router {
    let api_router = Router::new()
        .merge(auth_routes())
        .merge(sys_user_routes(state.clone()))
        .merge(user_routes(state.clone()))
        .merge(role_routes(state.clone()))
        .merge(menu_routes(state.clone()))
        .merge(org_routes(state.clone()))
        .merge(sys_auth_routes(state.clone()))
        .merge(sys_dict_routes(state.clone()))
        .merge(sys_dict_item_routes(state.clone()))
        .merge(sys_log_routes(state.clone()))
        .merge(dog_map_routes(state.clone()))
        .merge(dog_media_routes(state.clone()))
        .merge(dog_model_routes(state.clone()))
        .merge(dog_config_routes(state.clone()))
        .merge(dog_command_routes(state.clone()))
        .merge(dog_services_routes(state.clone()))
        .merge(dog_statistics_routes(state.clone()));

    Router::new()
        .nest("/api", api_router)
        .layer(TraceLayer::new_for_http())
        .layer(from_fn_with_state(state.clone(), audit_log_middleware))
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state)
}
