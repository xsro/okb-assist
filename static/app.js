// API helper
const api = {
    token: localStorage.getItem('token'),

    headers() {
        const h = { 'Content-Type': 'application/json' };
        if (this.token) h['Authorization'] = `Bearer ${this.token}`;
        return h;
    },

    async request(url, options = {}) {
        const res = await fetch(url, {
            ...options,
            headers: { ...this.headers(), ...options.headers },
        });
        if (res.status === 401) {
            localStorage.removeItem('token');
            window.location.href = '/assist/login';
            return;
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
            headers: { 'Authorization': `Bearer ${this.token}` },
            body: formData,
        });
        if (!res.ok) {
            const err = await res.json().catch(() => ({ detail: '上传失败' }));
            throw new Error(err.detail || '上传失败');
        }
        return res.json();
    },

    setToken(token) {
        this.token = token;
        localStorage.setItem('token', token);
    },

    clearToken() {
        this.token = null;
        localStorage.removeItem('token');
    }
};

// Toast notification
function showToast(message, type = 'info') {
    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.textContent = message;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

// Check auth
function checkAuth() {
    if (!api.token && !window.location.href.includes('/login')) {
        window.location.href = '/assist/login';
        return false;
    }
    return true;
}

// Update nav based on auth
function updateNav() {
    const nav = document.getElementById('nav');
    if (!nav) return;

    if (api.token) {
        nav.innerHTML = `
            <a href="/assist/">文献列表</a>
            <a href="/assist/upload">上传</a>
            <a href="/assist/admin">管理</a>
            <a href="#" onclick="logout()">退出</a>
        `;
    } else {
        nav.innerHTML = `<a href="/assist/login">登录</a>`;
    }
}

function logout() {
    api.clearToken();
    window.location.href = '/assist/login';
}

// Status display
function statusBadge(status) {
    const labels = {
        'uploaded': '已上传',
        'markdown_done': '已解析',
        'meta_done': '已提取',
        'indexed': '已索引',
    };
    return `<span class="badge badge-${status}">${labels[status] || status}</span>`;
}

// Init on page load
document.addEventListener('DOMContentLoaded', () => {
    updateNav();
});
