<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import { message } from "@/utils/message";
import aesUtils from "@/utils/aes";
import { getMine, updatePassword } from "@/api/user";
import Avatar from "@/assets/user.jpg";

import UserLine from "~icons/ri/user-3-line";
import MailLine from "~icons/ri/mail-line";
import PhoneLine from "~icons/ri/phone-line";
import FileListLine from "~icons/ri/file-list-3-line";

defineOptions({
  name: "AccountSettings"
});

const activeTab = ref("info");
const loading = ref(false);
const pwdFormRef = ref();
const mine = ref({
  avatar: "",
  username: "",
  nickname: "",
  email: "",
  phone: "",
  description: ""
});

// 头像为空时使用默认头像
const userAvatar = computed(() => mine.value.avatar || Avatar);

const infoItems = computed(() => [
  {
    icon: UserLine,
    color: "#41b6ff",
    label: "账号",
    value: mine.value.username || "-"
  },
  {
    icon: UserLine,
    color: "#7846e5",
    label: "昵称",
    value: mine.value.nickname || "-"
  },
  {
    icon: MailLine,
    color: "#26ce83",
    label: "邮箱",
    value: mine.value.email || "-"
  },
  {
    icon: PhoneLine,
    color: "#e85f33",
    label: "联系电话",
    value: mine.value.phone || "-"
  },
  {
    icon: FileListLine,
    color: "#a259ff",
    label: "简介",
    value: mine.value.description || "-"
  }
]);

const pwdForm = ref({
  oldPassword: "",
  newPassword: "",
  confirmPassword: ""
});

const pwdRules = {
  oldPassword: [{ required: true, message: "请输入原密码", trigger: "blur" }],
  newPassword: [
    { required: true, message: "请输入新密码", trigger: "blur" },
    { min: 6, message: "密码长度不能少于 6 位", trigger: "blur" }
  ],
  confirmPassword: [
    { required: true, message: "请再次输入新密码", trigger: "blur" },
    {
      validator: (_rule, value, callback) => {
        if (value && value !== pwdForm.value.newPassword) {
          callback(new Error("两次输入的新密码不一致"));
        } else {
          callback();
        }
      },
      trigger: "blur"
    }
  ]
};

function getMineInfo() {
  loading.value = true;
  getMine()
    .then(res => {
      if (res?.success) {
        mine.value = { ...mine.value, ...res.data };
      }
    })
    .finally(() => {
      loading.value = false;
    });
}

function submitPwd() {
  pwdFormRef.value?.validate(valid => {
    if (!valid) return;
    updatePassword({
      old_password: pwdForm.value.oldPassword,
      // 新密码使用与登录一致的 AES 加密后提交
      password: aesUtils.encode(pwdForm.value.newPassword, "")
    }).then(res => {
      if (res?.success) {
        message("密码修改成功", { type: "success" });
        pwdForm.value = {
          oldPassword: "",
          newPassword: "",
          confirmPassword: ""
        };
        pwdFormRef.value?.resetFields();
      }
    });
  });
}

onMounted(getMineInfo);
</script>

<template>
  <div v-loading="loading" class="main">
    <!-- 头部资料卡片 -->
    <el-card class="profile-card" shadow="never">
      <div class="profile-header">
        <div class="avatar-wrap">
          <el-avatar :size="72" :src="userAvatar" />
        </div>
        <div class="profile-meta pl-5">
          <div class="flex items-center gap-2">
            <span class="profile-name">
              {{ mine.nickname || mine.username || "未设置昵称" }}
            </span>
            <el-tag size="small" effect="dark" round disable-transitions
              >账户</el-tag
            >
          </div>
          <div class="profile-account">@{{ mine.username || "-" }}</div>
          <p class="profile-desc">
            {{ mine.description || "这个人很懒，什么都没有留下" }}
          </p>
        </div>
      </div>
    </el-card>

    <!-- 内容卡片 -->
    <el-card class="content-card mt-4" shadow="never">
      <el-tabs v-model="activeTab" class="account-tabs">
        <!-- 基本信息 -->
        <el-tab-pane label="基本信息" name="info">
          <div class="info-grid">
            <div
              v-for="item in infoItems"
              :key="item.label"
              class="info-item group"
            >
              <div
                class="info-icon"
                :style="{
                  color: item.color,
                  backgroundColor: item.color + '1a'
                }"
              >
                <IconifyIconOffline :icon="item.icon" width="20" height="20" />
              </div>
              <div class="min-w-0">
                <p class="info-label">{{ item.label }}</p>
                <p class="info-value truncate">{{ item.value }}</p>
              </div>
            </div>
          </div>
        </el-tab-pane>

        <!-- 修改密码 -->
        <el-tab-pane label="修改密码" name="pwd">
          <el-form
            ref="pwdFormRef"
            :model="pwdForm"
            :rules="pwdRules"
            label-width="110px"
            class="pwd-form"
          >
            <el-form-item label="原密码" prop="oldPassword">
              <el-input
                v-model="pwdForm.oldPassword"
                type="password"
                show-password
                placeholder="请输入原密码"
                clearable
              />
            </el-form-item>
            <el-form-item label="新密码" prop="newPassword">
              <el-input
                v-model="pwdForm.newPassword"
                type="password"
                show-password
                placeholder="请输入新密码（不能少于 6 位）"
                clearable
              />
            </el-form-item>
            <el-form-item label="确认新密码" prop="confirmPassword">
              <el-input
                v-model="pwdForm.confirmPassword"
                type="password"
                show-password
                placeholder="请再次输入新密码"
                clearable
              />
            </el-form-item>
            <el-form-item>
              <el-button type="primary" class="submit-btn" @click="submitPwd">
                保存修改
              </el-button>
            </el-form-item>
          </el-form>
        </el-tab-pane>
      </el-tabs>
    </el-card>
  </div>
</template>

<style lang="scss" scoped>
.profile-card {
  --el-card-border-color: none;

  :deep(.el-card__body) {
    padding: 24px;
  }

  .profile-header {
    display: flex;
    align-items: center;
  }

  .avatar-wrap {
    position: relative;
    flex-shrink: 0;

    &::after {
      position: absolute;
      inset: -3px;
      content: "";
      border: 2px solid var(--el-color-primary);
      border-radius: 50%;
      opacity: 0.4;
    }
  }

  .profile-name {
    font-size: 20px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .profile-account {
    margin-top: 4px;
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }

  .profile-desc {
    margin-top: 8px;
    font-size: 13px;
    color: var(--el-text-color-regular);
  }
}

.content-card {
  --el-card-border-color: none;

  :deep(.el-card__body) {
    padding: 8px 24px 24px;
  }

  .account-tabs {
    :deep(.el-tabs__nav-wrap::after) {
      background-color: var(--el-border-color-lighter);
    }
  }
}

.info-grid {
  display: grid;
  grid-template-columns: repeat(1, 1fr);
  gap: 16px;
  max-width: 720px;
  padding: 8px 0;

  @media (width >= 768px) {
    grid-template-columns: repeat(2, 1fr);
  }

  .info-item {
    display: flex;
    gap: 14px;
    align-items: center;
    padding: 16px;
    background-color: var(--el-fill-color-light);
    border-radius: 12px;
    transition:
      transform 0.2s ease,
      box-shadow 0.2s ease;

    &:hover {
      box-shadow: 0 6px 16px rgb(0 0 0 / 6%);
      transform: translateY(-2px);
    }

    .info-icon {
      display: flex;
      flex-shrink: 0;
      align-items: center;
      justify-content: center;
      width: 42px;
      height: 42px;
      border-radius: 10px;
    }

    .info-label {
      margin-bottom: 2px;
      font-size: 12px;
      color: var(--el-text-color-secondary);
    }

    .info-value {
      font-size: 14px;
      font-weight: 500;
      color: var(--el-text-color-primary);
    }
  }
}

.pwd-form {
  max-width: 480px;
  padding: 12px 0;

  :deep(.el-input__wrapper) {
    border-radius: 8px;
  }

  .submit-btn {
    width: 160px;
    border-radius: 8px;
  }
}
</style>
