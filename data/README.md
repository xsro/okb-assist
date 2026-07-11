# 挂在xfs的U盘
sudo mount /dev/sdb1 /mnt/usb

# 修改U盘的权限
sudo chown -R $USER:$USER /mnt/usb
# 测试
touch /mnt/usb/a.txt


# 复制文件到U盘
rsync -av /home/a422/repo/okb-assist/uploads uploads/
sudo unmount -l /mnt/usb