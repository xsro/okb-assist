// API helper
const api = {
    getToken() {
        return localStorage.getItem('upload_token') || '';
    },

    setToken(token) {
        localStorage.setItem('upload_token', token);
    },

    headers(includeToken = false) {
        const h = { 'Content-Type': 'application/json' };
        if (includeToken) {
            h['X-Token'] = this.getToken();
        }
        return h;
    },

    async request(url, options = {}) {
        const res = await fetch(url, {
            ...options,
            headers: { ...this.headers(), ...options.headers },
        });
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
        'markdown_done': '已解析',
        'meta_done': '已提取',
        'indexed': '已索引',
    };
    return `<span class="badge badge-${status}">${labels[status] || status}</span>`;
}

// Init on page load
document.addEventListener('DOMContentLoaded', () => {
    // Update nav
    const nav = document.getElementById('nav');
    if (nav) {
        nav.innerHTML = `
            <a href="/assist/">文献列表</a>
            <a href="/assist/upload">上传</a>
            <a href="/assist/admin">管理</a>
        `;
    }
});
