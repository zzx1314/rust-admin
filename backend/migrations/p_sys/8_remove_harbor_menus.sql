-- @migration-name: remove_harbor_menus

-- The Harbor / app review features (and their frontend views) were removed.
-- Drop the menu entries that still pointed at the deleted `harbor` component so
-- the admin panel does not render a broken menu item.

UPDATE p_sys_menu
SET is_deleted = 1,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id IN (1103, 1104, 1105);

DELETE FROM p_sys_role_menu
WHERE menu_id IN (1103, 1104, 1105);
