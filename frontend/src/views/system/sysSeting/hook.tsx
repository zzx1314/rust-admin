import { onMounted, reactive, ref } from "vue";
import { ElMessage, type FormInstance, type FormRules } from "element-plus";
import { getSafePolicy, updateSafePolicy } from "@/api/system";
import { SUCCESS } from "@/api/base";

const defaultForm = () => ({
  type: "sys_security_policy",
  sysLoginMaxLockTime: "5分钟",
  sysLoginMaxTryCount: "5次",
  sysPassLength: 11,
  sysPassShortLength: 8,
  sysPassChange: "5天",
  sysOvertime: "30分钟",
  passCom: "密码是数字，字母组合"
});

export function useSysSeting() {
  const loading = ref(false);
  const addForm = ref(defaultForm());

  const rules = reactive<FormRules>({
    sysLoginMaxLockTime: [
      { required: true, message: "锁定时长必填", trigger: "change" }
    ],
    sysLoginMaxTryCount: [
      { required: true, message: "最大尝试次数必填", trigger: "change" }
    ],
    sysPassLength: [
      { required: true, message: "密码长度必填", trigger: "change" }
    ],
    sysPassShortLength: [
      { required: true, message: "密码长度必填", trigger: "change" }
    ],
    sysPassChange: [
      { required: true, message: "密码更换周期必填", trigger: "change" }
    ],
    sysOvertime: [
      { required: true, message: "超时时间必填", trigger: "change" }
    ],
    passCom: [{ required: true, message: "密码复杂度必填", trigger: "change" }]
  });

  /** 取消：放弃本地修改，恢复为服务端已保存的配置 */
  const cancel = (formRef?: FormInstance) => {
    formRef?.clearValidate();
    getSysSeting();
  };

  const getSysSeting = () => {
    loading.value = true;
    getSafePolicy()
      .then(res => {
        if (res.code === SUCCESS) {
          addForm.value = res.data;
        }
      })
      .finally(() => {
        loading.value = false;
      });
  };

  onMounted(() => {
    getSysSeting();
  });

  const addFormInfo = (formRef?: FormInstance) => {
    formRef?.validate(valid => {
      if (!valid) return;
      updateSafePolicy(addForm.value).then(res => {
        if (res.code === SUCCESS) {
          ElMessage.success("保存成功，新的配置将会在下次登录生效！");
        } else {
          ElMessage.error(res.msg);
        }
      });
    });
  };

  return {
    loading,
    addForm,
    rules,
    cancel,
    addFormInfo
  };
}
