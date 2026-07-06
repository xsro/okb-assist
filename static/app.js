// API helper
const api = {
    getToken() {
        return localStorage.getItem('upload_token') || '';
    },

    setToken(token) {
        localStorage.setItem('upload_token', token);
        // 同时存储到 cookie，这样页面请求也会自动携带
        document.cookie = `x_token=${token}; path=/assist; max-age=86400`;
    },

    removeToken() {
        localStorage.removeItem('upload_token');
        // 删除 cookie
        document.cookie = 'x_token=; path=/assist; max-age=0';
    },

    isAuthenticated() {
        return !!this.getToken();
    },

    headers() {
        const h = { 'Content-Type': 'application/json' };
        // 所有请求都携带token
        const token = this.getToken();
        if (token) {
            h['X-Token'] = token;
        }
        return h;
    },

    // 获取带token的URL
    urlWithToken(url) {
        const token = this.getToken();
        if (!token) return url;
        const separator = url.includes('?') ? '&' : '?';
        return `${url}${separator}token=${encodeURIComponent(token)}`;
    },

    async request(url, options = {}) {
        // 同时在header和URL中携带token
        const tokenUrl = this.urlWithToken(url);
        const res = await fetch(tokenUrl, {
            ...options,
            headers: { ...this.headers(), ...options.headers },
        });
        if (res.status === 401) {
            // 未授权，跳转到登录页面
            window.location.href = '/assist/login';
            throw new Error('请先登录');
        }
        if (!res.ok) {
            const err = await res.json().catch(() => ({ detail: '请求失败' }));
            throw new Error(err.detail || '请求失败');
        }
        return res.json();
    },

    get(url) { return this.request(url); },
    post(url, data) { return this.request(url, { method: 'POST', body: JSON.stringify(data) }); },
    put(url, data) { return this.request(url, { method: 'PUT', body: JSON.stringify(data) }); },
    delete(url) { return this.request(url, { method: 'DELETE' }); },

    async upload(url, file) {
        const formData = new FormData();
        formData.append('file', file);
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'X-Token': this.getToken() },
            body: formData,
        });
        if (res.status === 401) {
            window.location.href = '/assist/login';
            throw new Error('请先登录');
        }
        if (!res.ok) {
            const err = await res.json().catch(() => ({ detail: '上传失败' }));
            throw new Error(err.detail || '上传失败');
        }
        return res.json();
    },
};

// Toast notification
function showToast(message, type = 'info') {
    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.textContent = message;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

// Status display
function statusBadge(status) {
    const labels = {
        'uploaded': '已上传',
        'parsing': '解析中',
        'markdown_done': '已解析',
        'extracting': '提取中',
        'meta_done': '已提取',
        'indexing': '索引中',
        'indexed': '已索引',
        'error': '错误',
    };
    return `<span class="badge badge-${status}">${labels[status] || status}</span>`;
}

// Navigation
function renderNav() {
    const nav = document.getElementById('nav');
    if (!nav) return;

    const authenticated = api.isAuthenticated();

    // 无需认证的页面
    const publicLinks = `
        <a href="/assist/">文献列表</a>
        <a href="/assist/monitor">监控</a>
    `;

    // 需要认证的页面
    const authLinks = `
        <a href="/assist/upload">上传</a>
        <a href="/assist/tools">工具</a>
        <a href="/assist/admin">管理</a>
    `;

    // 登录/登出按钮
    const authButton = authenticated
        ? `<a href="#" onclick="logout()" style="margin-left:auto;color:var(--danger)">登出</a>`
        : `<a href="/assist/login" style="margin-left:auto">登录</a>`;

    nav.innerHTML = publicLinks + (authenticated ? authLinks : '') + authButton;
}

function logout() {
    api.removeToken();
    showToast('已登出', 'info');
    renderNav();
    // 如果当前页面需要认证，跳转到首页
    const path = window.location.pathname;
    const protectedPaths = ['/assist/upload', '/assist/tools', '/assist/admin', '/assist/doc/'];
    if (protectedPaths.some(p => path.startsWith(p))) {
        window.location.href = '/assist/';
    }
}

// Init on page load
document.addEventListener('DOMContentLoaded', () => {
    renderNav();
});
