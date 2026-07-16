// API helper
const api = {
    headers(includeJson = true) {
        return includeJson ? { 'Content-Type': 'application/json' } : {};
    },

    async request(url, options = {}) {
        const includeJson = !(options.body instanceof FormData);
        const res = await fetch(url, {
            ...options,
            credentials: 'same-origin',
            headers: { ...this.headers(includeJson), ...options.headers },
        });
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

        const res = await fetch(url, {
            method: 'POST',
            credentials: 'same-origin',
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

    nav.innerHTML = `
        <a href="/assist/">文献列表</a>
        <a href="/assist/upload">上传</a>
        <a href="/assist/tools">工具</a>
        <a href="/assist/admin">管理</a>
        <a href="/assist/config">配置</a>
    `;
}

// Init on page load
document.addEventListener('DOMContentLoaded', () => {
    renderNav();
});
