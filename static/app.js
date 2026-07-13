// API helper
const api = {
    authState: {
        checked: false,
        authenticated: false,
        token_authenticated: false,
        trusted_client: false,
        auth_required: true,
    },

    getCookieToken() {
        const prefix = 'x_token=';
        const cookie = document.cookie
            .split(';')
            .map(part => part.trim())
            .find(part => part.startsWith(prefix));
        if (!cookie) return '';
        return decodeURIComponent(cookie.slice(prefix.length));
    },

    getToken() {
        return localStorage.getItem('upload_token') || this.getCookieToken() || '';
    },

    setToken(token) {
        const normalized = (token || '').trim();
        if (!normalized) {
            this.removeToken();
            return;
        }

        localStorage.setItem('upload_token', normalized);
        // 页面导航无法带自定义 header，cookie 用来让受保护页面正常打开。
        document.cookie = `x_token=${encodeURIComponent(normalized)}; path=/assist; max-age=86400; SameSite=Lax`;
    },

    removeToken() {
        localStorage.removeItem('upload_token');
        document.cookie = 'x_token=; path=/assist; max-age=0; SameSite=Lax';
    },

    isAuthenticated() {
        return this.authState.authenticated || !!this.getToken();
    },

    hasAccess() {
        return this.isAuthenticated();
    },

    hasToken() {
        return !!this.getToken();
    },

    async refreshAuthStatus() {
        const token = this.getToken();
        const headers = {};
        if (token) {
            headers['X-Token'] = token;
        }

        const res = await fetch('/assist/api/auth/check', {
            headers,
            credentials: 'same-origin',
        });
        if (!res.ok) {
            this.authState = { ...this.authState, checked: true, authenticated: false };
            return this.authState;
        }

        const status = await res.json();
        this.authState = {
            checked: true,
            authenticated: !!status.authenticated,
            token_authenticated: !!status.token_authenticated,
            trusted_client: !!status.trusted_client,
            auth_required: status.auth_required !== false,
        };

        if (token && this.authState.auth_required && !this.authState.token_authenticated) {
            this.removeToken();
        }

        return this.authState;
    },

    headers(includeJson = true) {
        const h = includeJson ? { 'Content-Type': 'application/json' } : {};
        const token = this.getToken();
        if (token) {
            h['X-Token'] = token;
        }
        return h;
    },

    urlWithToken(url) {
        const token = this.getToken();
        if (!token) return url;
        const separator = url.includes('?') ? '&' : '?';
        return `${url}${separator}token=${encodeURIComponent(token)}`;
    },

    async request(url, options = {}) {
        const includeJson = !(options.body instanceof FormData);
        const res = await fetch(url, {
            ...options,
            credentials: 'same-origin',
            headers: { ...this.headers(includeJson), ...options.headers },
        });
        if (res.status === 401) {
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
    post(url, data) {
        return this.request(url, {
            method: 'POST',
            body: data === undefined ? undefined : JSON.stringify(data),
        });
    },
    put(url, data) { return this.request(url, { method: 'PUT', body: JSON.stringify(data) }); },
    delete(url) { return this.request(url, { method: 'DELETE' }); },

    async upload(url, file) {
        const formData = new FormData();
        formData.append('file', file);

        const token = this.getToken();
        const headers = token ? { 'X-Token': token } : {};
        const res = await fetch(url, {
            method: 'POST',
            headers,
            credentials: 'same-origin',
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

    const hasAccess = api.hasAccess();
    const hasToken = api.hasToken();

    const publicLinks = `
        <a href="/assist/">文献列表</a>
    `;

    const authLinks = `
        <a href="/assist/upload">上传</a>
        <a href="/assist/tools">工具</a>
        <a href="/assist/admin">管理</a>
        <a href="/assist/config">配置</a>
    `;

    const authButton = hasToken
        ? `<a href="#" onclick="logout()" style="margin-left:auto;color:var(--danger)">登出</a>`
        : (api.authState.auth_required ? `<a href="/assist/login" style="margin-left:auto">登录</a>` : '');

    nav.innerHTML = publicLinks + (hasAccess ? authLinks : '') + authButton;
}

function logout() {
    api.removeToken();
    showToast('已登出', 'info');
    renderNav();

    const path = window.location.pathname;
    if (api.authState.auth_required && path.startsWith('/assist') && path !== '/assist/login') {
        window.location.href = '/assist/login';
    }
}

// Init on page load
document.addEventListener('DOMContentLoaded', () => {
    renderNav();
    api.refreshAuthStatus()
        .then(renderNav)
        .catch(() => {});
});
