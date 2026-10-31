<template>
  <div class="mcp-setup-view">
    <h2>MCP 配置</h2>

    <section class="section">
      <h3>个人访问令牌（推荐）</h3>
      <p class="section-desc">
        每个用户可以拥有独立令牌，权限跟随自己的角色与「MCP 访问」开关；令牌泄露只需撤销自己那把，
        不影响其他人。明文只在创建时显示一次。
      </p>
      <ul class="mcp-hint-list">
        <li>
          在「<router-link to="/assist/account">账户</router-link>」页创建令牌（勾选“允许访问 MCP”）。
        </li>
        <li>HTTP 传输：<code>Authorization: Bearer okb_…</code>，端点 <code>/assist/mcp/stream</code>。</li>
        <li>
          未获得 MCP 权限时，请联系管理员在「<router-link to="/assist/users">用户管理</router-link
          >」里为你的账号打开 MCP。
        </li>
      </ul>
      <router-link class="btn btn-sm" to="/assist/account">前往账户页管理令牌</router-link>
    </section>

    <McpConfigPanel />
  </div>
</template>

<script setup lang="ts">
import McpConfigPanel from '@/components/McpConfigPanel.vue'
</script>

<style scoped>
.section-desc {
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 12px;
}
.mcp-hint-list {
  margin: 0 0 14px 18px;
  padding: 0;
  font-size: 13px;
  color: var(--text-secondary);
  line-height: 1.9;
}
.mcp-hint-list code {
  background: var(--bg);
  padding: 1px 5px;
  border-radius: 3px;
  font-size: 12px;
}
</style>
