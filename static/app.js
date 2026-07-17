// ── Token 管理 ─────────────────────────────────────────────────────────────
const tokenManager = {
    KEY: 'okb_token',

    get() {
        return localStorage.getItem(this.KEY) || '';
    },

    set(token) {
        localStorage.setItem(this.KEY, token);
    },

    clear() {
        localStorage.removeItem(this.KEY);
    },

    /** 尝试从 URL query 参数获取 token 并保存 */
    tryCaptureFromUrl() {
        const params = new URLSearchParams(window.location.search);
        const t = params.get('token');
        if (t) {
            this.set(t);
            // 移除 URL 中的 token 参数
            params.delete('token');
            const newSearch = params.toString();
            const newUrl = window.location.pathname + (newSearch ? '?' + newSearch : '') + window.location.hash;
            window.history.replaceState({}, '', newUrl);
        }
    },
};

// 页面加载时尝试从 URL 捕获 token
tokenManager.tryCaptureFromUrl();

// ── API helper ─────────────────────────────────────────────────────────────
const api = {
    headers(includeJson = true) {
        const h = includeJson ? { 'Content-Type': 'application/json' } : {};
        const token = tokenManager.get();
        if (token) h['X-Token'] = token;
        return h;
    },

    /** 给 URL 附加 token query 参数（用于不走 headers 的场景如 SSE） */
    withToken(url) {
        const token = tokenManager.get();
        if (!token) return url;
        const sep = url.includes('?') ? '&' : '?';
        return url + sep + 'token=' + encodeURIComponent(token);
    },

    async request(url, options = {}) {
        const includeJson = !(options.body instanceof FormData);
        const res = await fetch(url, {
            ...options,
            credentials: 'same-origin',
            headers: { ...this.headers(includeJson), ...options.headers },
        });
        if (res.status === 401) {
            tokenManager.clear();
            promptToken('Token 已失效，请重新输入');
            throw new Error('未授权：请提供有效的 Token');
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

        const headers = {};
        const token = tokenManager.get();
        if (token) headers['X-Token'] = token;

        const res = await fetch(url, {
            method: 'POST',
            credentials: 'same-origin',
            headers,
            body: formData,
        });
        if (res.status === 401) {
            tokenManager.clear();
            promptToken('Token 已失效，请重新输入');
            throw new Error('未授权：请提供有效的 Token');
        }
        if (!res.ok) {
            const err = await res.json().catch(() => ({ detail: '上传失败' }));
            throw new Error(err.detail || '上传失败');
        }
        return res.json();
    },
};

// ── Token 输入弹窗 ─────────────────────────────────────────────────────────
function promptToken(message = '请输入访问 Token') {
    if (document.getElementById('token-dialog')) return;

    const div = document.createElement('div');
    div.id = 'token-dialog';
    div.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.5);display:flex;align-items:center;justify-content:center;z-index:9999';
    div.innerHTML = `
        <div style="background:white;padding:2rem;border-radius:12px;max-width:380px;width:90%;box-shadow:0 8px 32px rgba(0,0,0,0.2)">
            <h3 style="margin:0 0 0.75rem 0;font-size:1.1rem">🔐 ${message}</h3>
            <input type="password" id="token-input" class="form-control" placeholder="输入 Token"
                   style="width:100%;margin-bottom:1rem;box-sizing:border-box"
                   onkeydown="if(event.key==='Enter')submitToken()">
            <div style="display:flex;justify-content:flex-end;gap:0.5rem">
                <button class="btn" onclick="document.getElementById('token-dialog').parentElement.remove()">取消</button>
                <button class="btn btn-primary" onclick="submitToken()">确认</button>
            </div>
        </div>
    `;
    document.body.appendChild(div);
    document.getElementById('token-input').focus();
}

function submitToken() {
    const input = document.getElementById('token-input');
    const token = input ? input.value.trim() : '';
    if (!token) return;
    tokenManager.set(token);
    const dialog = document.getElementById('token-dialog');
    if (dialog) dialog.parentElement.remove();
    // 刷新当前页面数据（如果页面有 load 函数）
    if (typeof loadDocuments === 'function') loadDocuments();
    if (typeof loadDocument === 'function') loadDocument();
    if (typeof loadConfig === 'function') loadConfig();
    showToast('Token 已更新', 'success');
}

// ── 首次访问自动检测 ──────────────────────────────────────────────────────
async function checkTokenOnLoad() {
    const token = tokenManager.get();
    // 检测 token 是否有效（用一个轻量级请求）
    try {
        const res = await fetch('/assist/api/config', {
            credentials: 'same-origin',
            headers: token ? { 'X-Token': token } : {},
        });
        if (res.status === 401 && !token) {
            promptToken('请输入访问 Token');
        }
    } catch {}
}

// ── Toast ──────────────────────────────────────────────────────────────────
function showToast(message, type = 'info') {
    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.textContent = message;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

// ── Status ─────────────────────────────────────────────────────────────────
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

// ── Navigation ─────────────────────────────────────────────────────────────
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

// ── Init ───────────────────────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', () => {
    renderNav();
    checkTokenOnLoad();
});
