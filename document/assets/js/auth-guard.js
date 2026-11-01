/*!
 * 静态文档站的登录守卫。
 *
 * `static_dirs` 的挂载（`system.json` 里 `/document` → `../document`）默认需要凭证：
 * 未登录时后端对 `README.md` 之类的静态文件返回 401，docsify 会把它当成 404 显示，
 * 用户只看到「页面不存在」。这里先用一次探针请求确认身份，401/403 就整页跳到登录页，
 * 并把当前地址塞进 `?redirect=`，登录成功后自动跳回。
 */
(function () {
  var LOGIN_PREFIX = '/assist/login?redirect=';

  // 带时间戳绕开缓存，确保拿到的是真实的鉴权状态而不是缓存副本
  var probe = 'README.md?probe=' + Date.now();

  fetch(probe, { credentials: 'same-origin', cache: 'no-store' })
    .then(function (res) {
      if (res.status !== 401 && res.status !== 403) return;
      var target = location.pathname + location.search + location.hash;
      location.replace(LOGIN_PREFIX + encodeURIComponent(target));
    })
    .catch(function () {
      /* 网络异常就交给 docsify 自己报错，别把用户卡在白屏 */
    });
})();
