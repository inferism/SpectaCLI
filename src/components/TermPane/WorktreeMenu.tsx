import { ask, message } from "@tauri-apps/plugin-dialog";
import { openFolder } from "../../lib/ipc";
import { type AgentWorktree, useWorkspace } from "../../store/workspace";

/** What the user would run to bring the agent's work into the main branch. Spectacli never merges. */
export const mergeCommand = (projectPath: string, branch: string) => `git -C "${projectPath}" merge ${branch}`;

const text = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

/**
 * The agent's branch plus its worktree actions: open the folder, copy the
 * merge command, remove the worktree (closes the agent; the branch stays).
 */
export default function WorktreeMenu({ instanceId, projectPath, worktree }: { instanceId: string; projectPath: string; worktree: AgentWorktree }) {
  const removeWorktree = useWorkspace((s) => s.removeWorktree);

  const remove = async () => {
    try {
      try {
        await removeWorktree(instanceId);
      } catch (e) {
        if (!text(e).startsWith("dirty:")) throw e;
        const yes = await ask(
          `This worktree has uncommitted changes (${text(e).slice(6).trim()}). Removing it deletes them for good. The branch ${worktree.branch} is kept.`,
          { title: "Remove worktree?", kind: "warning", okLabel: "Remove anyway", cancelLabel: "Cancel" },
        );
        if (yes) await removeWorktree(instanceId, true);
      }
    } catch (e) {
      await message(text(e), { title: "Could not remove the worktree", kind: "error" });
    }
  };

  return (
    <>
      <span className="termpane-branch" title={`Worktree: ${worktree.path}`}>
        ⎇ {worktree.branch}
      </span>
      <button aria-label="Open worktree folder" title="Open folder" onClick={() => void openFolder(worktree.path).catch(() => {})}>
        ⌂
      </button>
      <button
        aria-label="Copy merge command"
        title="Copy the git command that merges this branch (run it yourself)"
        onClick={() => void navigator.clipboard.writeText(mergeCommand(projectPath, worktree.branch)).catch(() => {})}
      >
        ⧉
      </button>
      <button aria-label="Remove worktree" title="Close the agent and remove its worktree (the branch is kept)" onClick={() => void remove()}>
        🗑
      </button>
    </>
  );
}
