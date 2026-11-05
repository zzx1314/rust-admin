-- @migration-name: remove_workflow_menu

-- 工作流 page is not needed in the admin panel, so its menu entry and the
-- `查询工作流` button are retired. The backend `/api/dogWorkflow*` endpoints
-- stay in place (tasks still carry a `workflow_id`), only the UI entry goes
-- away.

UPDATE p_sys_menu
SET is_deleted = 1,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id IN (1209, 1219);

DELETE FROM p_sys_role_menu
WHERE menu_id IN (1209, 1219);
