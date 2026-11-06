import { deleteRole, getRoleList, saveRole, updateRole } from "@/api/system";
import type { PaginationProps } from "@pureadmin/table";
import { reactive, ref, computed, onMounted } from "vue";
import type { FormRules, FormInstance } from "element-plus";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import type { DogSearchField } from "@/components/DogUI/search";

export function useRole() {
  // ----变量定义-----
  const form = reactive({
    name: "",
    code: "",
    status: ""
  });
  const dataList = ref([]);
  const loading = ref(true);
  const dialogFormVisible = ref(false);
  const title = ref("");
  const pagination = reactive<PaginationProps>({
    total: 0,
    pageSize: 10,
    currentPage: 1,
    background: true
  });
  const addForm = reactive({
    value: {
      id: null,
      name: "",
      code: "",
      status: "",
      description: "",
      remarks: ""
    }
  });
  /** 查询栏字段：与设备、指令等列表页共用同一套查询栏组件 */
  const queryFields: DogSearchField[] = [
    { label: "名称", prop: "name" },
    { label: "编码", prop: "code" }
  ];
  const rules = reactive<FormRules>({
    name: [{ required: true, message: "名称必填", trigger: "blur" }],
    code: [{ required: true, message: "编码必填", trigger: "blur" }]
  });
  const columns: TableColumnList = [
    {
      type: "selection",
      width: 55,
      align: "left",
      fixed: "left",
      label: "勾选列"
    },
    {
      label: "序号",
      type: "index",
      width: 70,
      fixed: "left"
    },
    {
      label: "编码",
      prop: "code",
      minWidth: 100
    },
    {
      label: "名称",
      prop: "name",
      minWidth: 120
    },
    {
      label: "角色描述",
      prop: "description",
      minWidth: 150
    },
    {
      label: "备注",
      prop: "remarks",
      minWidth: 150
    },
    {
      label: "创建时间",
      minWidth: 180,
      prop: "createTime"
    },
    {
      label: "操作",
      fixed: "right",
      width: 180,
      slot: "operation"
    }
  ];
  const buttonClass = computed(() => {
    return [
      "!h-[20px]",
      "reset-margin",
      "!text-gray-500",
      "dark:!text-white",
      "dark:hover:!text-primary"
    ];
  });

  // -----方法定义---
  // 修改
  function handleUpdate(row) {
    console.log(row);
    const roleInfo = JSON.stringify(row);
    addForm.value = JSON.parse(roleInfo);
    openDia("修改角色");
  }
  // 删除
  function handleDelete(row) {
    console.log(row);
    deleteRole(row.id).then(res => {
      if (res.code === SUCCESS) {
        message("删除成功！", { type: "success" });
        onSearch();
      } else {
        message(res.msg, { type: "error" });
      }
    });
  }

  function handleSizeChange(val: number) {
    console.log(`${val} items per page`);
  }

  function handleCurrentChange(val: number) {
    console.log(`current page: ${val}`);
  }

  function handleSelectionChange(val) {
    console.log("handleSelectionChange", val);
  }
  // 查询
  async function onSearch() {
    loading.value = true;
    const page = {
      size: pagination.pageSize,
      current: pagination.currentPage
    };
    const query = {
      ...page,
      ...form
    };
    const { data } = await getRoleList(query);
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }
  const resetForm = formEl => {
    if (!formEl) return;
    formEl.clearValidate();
  };
  /** 查询栏重置：清空查询条件后重新查询 */
  function resetQuery() {
    form.name = "";
    form.code = "";
    form.status = "";
    onSearch();
  }
  // 取消
  function cancel() {
    addForm.value = {
      id: null,
      name: "",
      code: "",
      status: "",
      description: "",
      remarks: ""
    };
    dialogFormVisible.value = false;
    onSearch();
  }
  // 保存
  const submitForm = async (formEl: FormInstance | undefined) => {
    if (!formEl) return;
    await formEl.validate((valid, fields) => {
      if (valid) {
        console.log(addForm.value);
        if (addForm.value.id) {
          // 修改
          updateRole(addForm.value).then(res => {
            if (res.code === SUCCESS) {
              message("修改成功！", { type: "success" });
              cancel();
            } else {
              message(res.msg, { type: "error" });
            }
          });
        } else {
          // 新增
          saveRole(addForm.value).then(res => {
            if (res.code === SUCCESS) {
              message("添加成功！", { type: "success" });
              cancel();
            } else {
              message(res.msg, { type: "error" });
            }
          });
        }
      } else {
        console.log("error submit!", fields);
      }
    });
  };
  // 打开弹框
  function openDia(param, formEl?) {
    dialogFormVisible.value = true;
    title.value = param;
    resetForm(formEl);
  }

  onMounted(() => {
    onSearch();
  });

  return {
    form,
    queryFields,
    loading,
    columns,
    dataList,
    pagination,
    buttonClass,
    dialogFormVisible,
    title,
    addForm,
    rules,
    onSearch,
    resetForm,
    resetQuery,
    handleUpdate,
    handleDelete,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    cancel,
    submitForm,
    openDia
  };
}
