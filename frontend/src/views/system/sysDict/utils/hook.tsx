import dayjs from "dayjs";
import DictForm from "../form/dict.vue";
import editForm from "../form/index.vue";
import { message } from "@/utils/message";
import { ElMessageBox } from "element-plus";
import { usePublicHooks } from "../../hooks";
import { h, ref, reactive, onMounted } from "vue";
import { addDialog } from "@/components/ReDialog";
import { deviceDetection } from "@pureadmin/utils";
import type { FormItemProps } from "../utils/types";
import type { PaginationProps } from "@pureadmin/table";

export function useDict() {
  // 左侧字典树的id
  const dictId = ref("");
  const formRef = ref();
  const dataList = ref([]);
  const loading = ref(true);
  const switchLoadMap = ref({});
  const { switchStyle } = usePublicHooks();
  const treeData = ref([]);
  const treeLoading = ref(true);
  const pagination = reactive<PaginationProps>({
    total: 0,
    pageSize: 10,
    currentPage: 1,
    background: true
  });
  const columns: TableColumnList = [
    {
      label: "字典标签",
      prop: "label",
      minWidth: 130,
      cellRenderer: scope => (
        <el-button size="small" color={scope.row.color}>
          {scope.row.label}
        </el-button>
      )
    },
    {
      label: "字典值",
      prop: "value",
      minWidth: 130
    },
    {
      label: "状态",
      prop: "status",
      minWidth: 90,
      cellRenderer: scope => (
        <el-switch
          size={scope.props.size === "small" ? "small" : "default"}
          loading={switchLoadMap.value[scope.index]?.loading}
          v-model={scope.row.status}
          active-value={1}
          inactive-value={0}
          active-text="已启用"
          inactive-text="已停用"
          inline-prompt
          style={switchStyle.value}
          onChange={() => onChange(scope as any)}
        />
      )
    },
    {
      sortable: true,
      label: "排序",
      minWidth: 90,
      prop: "sort"
    },
    {
      label: "备注",
      minWidth: 90,
      prop: "remark"
    },
    {
      label: "创建时间",
      minWidth: 90,
      prop: "createTime",
      formatter: ({ createTime }) =>
        dayjs(createTime).format("YYYY-MM-DD HH:mm:ss")
    },
    {
      label: "操作",
      fixed: "right",
      width: 180,
      slot: "operation"
    }
  ];

  function onChange({ row, index }) {
    ElMessageBox.confirm(
      `确定要<strong>${
        row.status === 0 ? "停用" : "启用"
      }</strong><strong style='color:var(--el-color-primary)'>${
        row.label
      }</strong>字典标签吗?`,
      "系统提示",
      {
        confirmButtonText: "确定",
        cancelButtonText: "取消",
        type: "warning",
        dangerouslyUseHTMLString: true,
        draggable: true
      }
    )
      .then(() => {
        switchLoadMap.value[index] = Object.assign(
          {},
          switchLoadMap.value[index],
          {
            loading: true
          }
        );
        setTimeout(() => {
          switchLoadMap.value[index] = Object.assign(
            {},
            switchLoadMap.value[index],
            {
              loading: false
            }
          );
          message("已成功修改状态", {
            type: "success"
          });
        }, 300);
      })
      .catch(() => {
        row.status === 0 ? (row.status = 1) : (row.status = 0);
      });
  }

  function handleDelete(row) {
    message(`您删除了字典标签为${row.label}的这条数据`, { type: "success" });
    onSearch();
  }

  function handleSizeChange(val: number) {
    console.log(`${val} items per page`);
  }

  function handleCurrentChange(val: number) {
    console.log(`current page: ${val}`);
  }

  async function onSearch() {
    loading.value = true;
    /*const { data } = await getDictDetail({ dictId: dictId.value });
    dataList.value = data.list;
    pagination.total = data.total;
    pagination.pageSize = data.pageSize;
    pagination.currentPage = data.currentPage;*/

    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  function onTreeSelect({ id, selected }) {
    dictId.value = selected ? id : "";
    onSearch();
  }

  function openDialog(title = "新增", row?: FormItemProps) {
    addDialog({
      title: `${title}字典详情`,
      props: {
        formInline: {
          title,
          label: row?.label ?? "",
          value: row?.value ?? "",
          color: row?.color ?? "#6abe39",
          sort: row?.sort ?? 999,
          status: row?.status ?? 1,
          remark: row?.remark ?? ""
        }
      },
      width: "32%",
      draggable: true,
      fullscreen: deviceDetection(),
      fullscreenIcon: true,
      closeOnClickModal: false,
      contentRenderer: () => h(editForm, { ref: formRef, formInline: null }),
      beforeSure: (done, { options }) => {
        const FormRef = formRef.value.getRef();
        const curData = options.props.formInline as FormItemProps;
        function chores() {
          message(`您${title}了字典标签为${curData.label}的这条数据`, {
            type: "success"
          });
          done(); // 关闭弹框
          onSearch(); // 刷新表格数据
        }
        FormRef.validate(valid => {
          if (valid) {
            console.log("curData", curData);
            // 表单规则校验通过
            if (title === "新增") {
              // 实际开发先调用新增接口，再进行下面操作
              chores();
            } else {
              // 实际开发先调用修改接口，再进行下面操作
              chores();
            }
          }
        });
      }
    });
  }

  function openDictDialog(title = "新增", row?: any) {
    addDialog({
      title: `${title}字典`,
      props: {
        formInline: {
          title,
          name: row?.name ?? "",
          code: row?.code ?? "",
          remark: row?.remark ?? ""
        }
      },
      width: "32%",
      draggable: true,
      fullscreen: deviceDetection(),
      fullscreenIcon: true,
      closeOnClickModal: false,
      contentRenderer: () => h(DictForm, { ref: formRef, formInline: null }),
      beforeSure: (done, { options }) => {
        const FormRef = formRef.value.getRef();
        const curData = options.props.formInline;
        function chores() {
          message(`您${title}了字典名称为${curData.name}的这条数据`, {
            type: "success"
          });
          done(); // 关闭弹框
          getDictTreeData(); // 刷新左侧字典树
        }
        FormRef.validate(valid => {
          if (valid) {
            console.log("curData", curData);
            // 表单规则校验通过
            if (title === "新增") {
              // 实际开发先调用新增接口，再进行下面操作
              chores();
            } else {
              // 实际开发先调用修改接口，再进行下面操作
              chores();
            }
          }
        });
      }
    });
  }

  /** 字典管理-左侧树数据 */
  async function getDictTreeData() {
    treeLoading.value = true;
    /*const { data } = await getDictTree();
    setTimeout(() => {
      treeData.value = data;
      treeLoading.value = false;
    }, 200);*/
  }

  onMounted(() => {
    getDictTreeData();
    onSearch();
  });

  return {
    dictId,
    loading,
    columns,
    dataList,
    treeData,
    treeLoading,
    pagination,
    deviceDetection,
    onSearch,
    openDialog,
    onTreeSelect,
    handleDelete,
    openDictDialog,
    getDictTreeData,
    handleSizeChange,
    handleCurrentChange
  };
}
