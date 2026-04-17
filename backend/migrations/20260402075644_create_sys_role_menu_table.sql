CREATE TABLE IF NOT EXISTS sys_role_menu (
    role_id TEXT NOT NULL,
    menu_id TEXT NOT NULL,
    PRIMARY KEY (role_id, menu_id),
    FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE,
    FOREIGN KEY (menu_id) REFERENCES menus(id) ON DELETE CASCADE
);
