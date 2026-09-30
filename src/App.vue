<script setup lang="ts"></script>

<template>
  <main class="shell">
    <h1 class="shell__title">
      Ageminal
    </h1>
    <p class="shell__lead">
      窗口与前端骨架已就位。
    </p>
    <p class="shell__hint">
      下一步：命令与类型边界（#51）、应用外壳与三栏布局（#55）。
    </p>
  </main>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 8px;
}

.shell__title {
  margin: 0;
  font-size: 28px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.shell__lead {
  margin: 0;
  color: var(--fg);
}

.shell__hint {
  margin: 0;
  color: var(--fg-muted);
  font-size: 13px;
}
</style>
