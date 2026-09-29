# udisks2 U 盘自动挂载配置指南

## 概述

本系统使用 **udisks2** 实现 U 盘的自动检测与挂载。这是大多数桌面 Linux 发行版的默认方案，无需安装额外软件。

工作机制：

1. **udev** 检测到 USB 存储设备插入，触发内核事件
2. **udisks2** 守护进程通过 D-Bus 接收事件，识别分区和文件系统
3. 根据 polkit 权限策略判断是否允许挂载
4. 允许后，以当前用户身份挂载到 `/media/<用户名>/<卷标>/` 目录
5. **GVFS**（`gvfs-udisks2-volume-monitor`）在文件管理器中显示该设备

## 服务状态

### 查看服务是否运行

```bash
systemctl status udisks2
```

预期输出应包含 `active (running)`。

### 查看当前已挂载的 U 盘

```bash
mount | grep /media/
```

示例输出：

```
/dev/sda1 on /media/orangepi/CCSICC type exfat (rw,relatime,uhelper=udisks2)
/dev/sdb1 on /media/orangepi/FAE6-F0B9 type exfat (rw,relatime,uhelper=udisks2)
```

挂载路径格式为 `/media/<用户名>/<卷标>`。`uhelper=udisks2` 标记表明此挂载由 udisks2 管理。

### 查看相关进程

```bash
pgrep -af "udisks2"
```

应有 `udisksd`（核心守护进程）和 `gvfs-udisks2-volume-monitor`（用户空间卷监视器）两个进程。

## 配置文件

### udisks2 主配置

| 文件 | 说明 |
|------|------|
| `/etc/udisks2/udisks2.conf` | 主配置文件 |
| `/etc/udisks2/mount_options.conf` | 自定义挂载选项（默认不存在，需手动创建） |
| `/etc/udisks2/mount_options.conf.example` | 挂载选项配置示例 |

当前主配置内容（`/etc/udisks2/udisks2.conf`）：

```ini
[udisks2]
# 加载所有模块
modules=*
# 按需加载（而非启动时全部加载）
modules_load_preference=ondemand

[defaults]
# LUKS 加密默认使用 LUKS1 格式
encryption=luks1
```

### 权限策略（polkit）

| 文件 | 说明 |
|------|------|
| `/usr/share/polkit-1/actions/org.freedesktop.UDisks2.policy` | udisks2 的 polkit 权限策略 |

关键操作 `org.freedesktop.udisks2.filesystem-mount` 的权限策略：

| 场景 | 策略 |
|------|------|
| 本地活动会话（`implicit active`） | **允许**（`yes`） — 插上即自动挂载 |
| 非活动会话（`implicit inactive`） | 需要管理员密码（`auth_admin`） |
| 远程/无会话（`implicit any`） | 需要管理员密码（`auth_admin`） |

这意味着在登录桌面环境后插入 U 盘会自动挂载，无需密码。

## 自定义挂载选项

如需修改 U 盘挂载时的默认参数（如强制只读、禁用执行等），可创建 `/etc/udisks2/mount_options.conf` 文件。

### 常用配置示例

#### 全局默认只读挂载

```ini
[defaults]
defaults=ro
allow=exec,noexec,nodev,nosuid,atime,noatime,nodiratime,relatime,ro,rw,sync,dirsync,noload
```

#### 为特定 U 盘设置选项（按 UUID）

先获取 UUID：

```bash
blkid /dev/sda1
```

然后在 `mount_options.conf` 中配置：

```ini
[/dev/disk/by-uuid/18afd8f0-0d86-4d96-8de0-5f92d2ee9800]
vfat_defaults=uid=$UID,gid=$GID,noexec
```

#### 为特定文件系统类型设置

```ini
# FAT32 默认选项
vfat_defaults=uid=$UID,gid=$GID,shortname=mixed,utf8=1,showexec,flush
vfat_allow=uid=$UID,gid=$GID,flush,utf8,shortname,umask,dmask,fmask,codepage,iocharset,usefree,showexec

# exFAT 默认选项（当前系统常用）
exfat_defaults=uid=$UID,gid=$GID,iocharset=utf8,errors=remount-ro
exfat_allow=uid=$UID,gid=$GID,dmask,errors,fmask,iocharset,namecase,umask

# NTFS 默认选项
ntfs_defaults=uid=$UID,gid=$GID,windows_names
ntfs_allow=uid=$UID,gid=$GID,umask,dmask,fmask,locale,norecover,ignore_case,windows_names,compression,nocompression,big_writes,nls,nohidden,sys_immutable,sparse,showmeta,prealloc
```

### 内置默认选项（无需配置时生效）

若 `/etc/udisks2/mount_options.conf` 不存在，udisks2 使用编译内置的默认值：

| 文件系统 | 默认挂载选项 |
|---------|-------------|
| `vfat` | `uid=$UID,gid=$GID,shortname=mixed,utf8=1,showexec,flush` |
| `exfat` | `uid=$UID,gid=$GID,iocharset=utf8,errors=remount-ro` |
| `ntfs` | `uid=$UID,gid=$GID,windows_names` |
| `ext2/3/4` | `errors=remount-ro` |
| `iso9660` | `uid=$UID,gid=$GID,iocharset=utf8,mode=0400,dmode=0500` |
| `btrfs` | 无特殊默认值（允许 `compress`、`discard` 等选项） |

### 配置格式说明

- `*_defaults`：该文件系统类型的**默认挂载选项**，会与 `[defaults]` 中的 `defaults=` 合并
- `*_allow`：该文件系统允许用户**自定义传入**的选项白名单
- `[defaults]` 中的 `defaults=`：**全局默认选项**，应用于所有文件系统
- `[defaults]` 中的 `allow=`：**全局允许选项**白名单
- `$UID` / `$GID`：运行时自动替换为当前用户的 UID 和 GID
- 支持按 UUID 或卷标匹配：`[/dev/disk/by-uuid/<UUID>]` 或 `[/dev/disk/by-label/<LABEL>]`

## 常见操作

### 手动挂载（udisks2 方式）

```bash
# 通过块设备路径
udisksctl mount -b /dev/sda1

# 通过块设备路径，指定文件系统类型
udisksctl mount -b /dev/sda1 -t exfat

# 查看块设备信息
udisksctl info -b /dev/sda1
```

### 手动卸载（udisks2 方式）

```bash
udisksctl unmount -b /dev/sda1

# 强制卸载
udisksctl unmount -b /dev/sda1 --force

# 弹出设备（卸载 + 断电）
udisksctl power-off -b /dev/sda1
```

### 查看所有块设备

```bash
lsblk
```

## 故障排查

### U 盘插入后未自动挂载

1. 检查 udisks2 服务是否在运行：
   ```bash
   systemctl status udisks2
   ```

2. 检查内核是否检测到设备：
   ```bash
   dmesg | tail -20 | grep -iE "usb|sd[a-z]"
   lsblk
   ```

3. 检查用户是否在相关组中：
   ```bash
   groups
   ```
   用户需要属于 `plugdev` 组。添加方式：
   ```bash
   sudo usermod -aG plugdev $USER
   # 重新登录后生效
   ```

4. 尝试手动挂载测试：
   ```bash
   udisksctl mount -b /dev/sda1
   ```
   观察错误信息。

### 修改配置文件后不生效

- `mount_options.conf` 的修改**立即生效**，无需重启服务
- `udisks2.conf` 的修改需重启服务：
  ```bash
  sudo systemctl restart udisks2
  ```

### 权限问题

若插拔 U 盘时提示认证对话框：

1. 确认用户属于 `plugdev` 组
2. 确认 `polkit` 服务运行正常：
   ```bash
   systemctl status polkit
   ```
3. 检查 polkit 策略是否被覆盖（查看 `/etc/polkit-1/localauthority/` 下是否有自定义规则）

## 参考链接

- [udisks2 官方文档](https://github.com/storaged-project/udisks)
- [udisks2 挂载选项参考](http://storaged.org/doc/udisks2-api/latest/mount_options.html)
- [polkit 参考](https://www.freedesktop.org/software/polkit/docs/latest/)