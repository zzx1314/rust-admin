import type { PlusColumn } from "plus-pro-components";
import { dogDeviceGetAll } from "@/api/dogDevice";
import { dogRouteGetAll } from "@/api/dogRoute";

export function useCollectorTaskForm() {
  const columnsForm: PlusColumn[] = [
    {
      label: "任务名称",
      prop: "taskName",
      valueType: "copy"
    },
    {
      label: "设备信息",
      prop: "deviceId",
      valueType: "select",
      options: async () => {
        const res = await dogDeviceGetAll();
        const devices = (res.data as Array<any>) || [];
        return devices.map(d => ({
          label: `${d.deviceId} (${d.model || ""})`,
          value: d.id
        }));
      }
    },
    {
      label: "航线信息",
      prop: "routeId",
      valueType: "select",
      options: async () => {
        const res = await dogRouteGetAll();
        const routes = (res.data as Array<any>) || [];
        return routes.map(r => ({
          label: r.routeName,
          value: r.id
        }));
      }
    },
    {
      label: "任务描述",
      prop: "taskDescription",
      valueType: "textarea"
    }
  ];

  const columnsQueryForm: PlusColumn[] = [
    {
      label: "任务名称",
      prop: "taskName",
      valueType: "copy",
      colProps: { span: 4 },
      formItemProps: { style: { width: "100%" } }
    },
    {
      label: "设备ID",
      prop: "deviceIdString",
      valueType: "copy",
      colProps: { span: 4 },
      formItemProps: { style: { width: "100%" } }
    },
    {
      label: "航线名称",
      prop: "routeName",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "任务状态",
      prop: "taskStatus",
      valueType: "select",
      options: [
        { label: "未开始", value: "未开始" },
        { label: "进行中", value: "进行中" },
        { label: "已完成", value: "已完成" },
        { label: "失败", value: "失败" }
      ],
      colProps: { span: 5 },
      formItemProps: { style: { width: "230px" } }
    },
    {
      label: "开始时间",
      prop: "beginTime",
      valueType: "date-picker",
      formItemProps: { style: { width: "250px" } },
      fieldProps: { type: "date", valueFormat: "YYYY-MM-DD HH:mm:ss" },
      colProps: { span: 5 }
    },
    {
      label: "结束时间",
      prop: "endTime",
      valueType: "date-picker",
      formItemProps: { style: { width: "250px" } },
      fieldProps: { type: "date", valueFormat: "YYYY-MM-DD" },
      colProps: { span: 5 }
    }
  ];

  return { columnsForm, columnsQueryForm };
}
