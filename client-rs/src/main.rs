//! OKB-Assist 客户端工具
//!
//! 子命令:
//!   zotero <csv_file>   — 从 Zotero 导出的 CSV 导入文献
//!   dir    <dir>        — 扫描目录中的 PDF，与服务器比对后交互上传
//!   sync                — 按 `~/.okb/client.json` 的 `sync` 配置同步网盘

mod dir;
mod sync;
mod util;
mod zotero;

use anyhow::Result;
use clap::Parser;

use dir::{run_dir, DirArgs};
use sync::run_sync;
use zotero::{run_zotero, ZoteroArgs};

// ==================== CLI 参数 ====================

#[derive(Parser, Debug)]
#[command(
    name = "okb-client",
    about = "OKB-Assist 客户端工具",
    version,
    subcommand_required = true,
    arg_required_else_help = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    /// 从 Zotero 导出的 CSV 导入文献
    Zotero(ZoteroArgs),
    /// 扫描目录中的 PDF，与服务器比对后交互上传
    Dir(DirArgs),
    /// 按 ~/.okb/client.json 的 sync 配置同步网盘（个人网盘 + 公有文件夹，无命令行参数）
    Sync(SyncArgs),
}

/// `sync` 子命令：所有配置（本地目录、网盘目录、同步方向、令牌）均来自客户端配置文件
#[derive(Parser, Debug)]
struct SyncArgs {}


// ==================== 入口 ====================

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Zotero(args) => run_zotero(args).await,
        Command::Dir(args) => run_dir(args).await,
        Command::Sync(_) => run_sync().await,
    }
}
