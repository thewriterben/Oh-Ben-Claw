// ── GitHub Live Data ──────────────────────────────────────────────────────────
// Fetches live data from the Oh-Ben-Claw and OBC-deployment-generator repos.

const OBC_REPO = 'thewriterben/Oh-Ben-Claw';
const OBC_GEN_REPO = 'thewriterben/OBC-deployment-generator';
const GITHUB_API = 'https://api.github.com';
const GITHUB_RAW = 'https://raw.githubusercontent.com';

export interface RepoInfo {
  latestTag: string | null;
  latestCommit: string | null;
  latestCommitDate: string | null;
  description: string | null;
  stars: number;
  openIssues: number;
}

export interface LiveData {
  obcRepo: RepoInfo;
  obcGenRepo: RepoInfo;
  fetchedAt: Date;
}

async function fetchRepoInfo(repo: string): Promise<RepoInfo> {
  try {
    // Fetch basic repo info
    const repoRes = await fetch(`${GITHUB_API}/repos/${repo}`, {
      headers: { Accept: 'application/vnd.github.v3+json' },
    });
    const repoData = repoRes.ok ? await repoRes.json() : null;

    // Fetch latest release tag
    const releaseRes = await fetch(`${GITHUB_API}/repos/${repo}/releases/latest`, {
      headers: { Accept: 'application/vnd.github.v3+json' },
    });
    const releaseData = releaseRes.ok ? await releaseRes.json() : null;

    // Fetch latest commit on main
    const commitRes = await fetch(`${GITHUB_API}/repos/${repo}/commits/main`, {
      headers: { Accept: 'application/vnd.github.v3+json' },
    });
    const commitData = commitRes.ok ? await commitRes.json() : null;

    return {
      latestTag: releaseData?.tag_name ?? null,
      latestCommit: commitData?.sha?.slice(0, 7) ?? null,
      latestCommitDate: commitData?.commit?.author?.date ?? null,
      description: repoData?.description ?? null,
      stars: repoData?.stargazers_count ?? 0,
      openIssues: repoData?.open_issues_count ?? 0,
    };
  } catch {
    return {
      latestTag: null,
      latestCommit: null,
      latestCommitDate: null,
      description: null,
      stars: 0,
      openIssues: 0,
    };
  }
}

export async function fetchLiveData(): Promise<LiveData> {
  const [obcRepo, obcGenRepo] = await Promise.all([
    fetchRepoInfo(OBC_REPO),
    fetchRepoInfo(OBC_GEN_REPO),
  ]);

  return {
    obcRepo,
    obcGenRepo,
    fetchedAt: new Date(),
  };
}

export async function fetchFileFromRepo(repo: string, path: string, branch = 'main'): Promise<string | null> {
  try {
    const res = await fetch(`${GITHUB_RAW}/${repo}/${branch}/${path}`);
    if (!res.ok) return null;
    return await res.text();
  } catch {
    return null;
  }
}

export async function fetchCargoVersion(): Promise<string | null> {
  const content = await fetchFileFromRepo(OBC_REPO, 'Cargo.toml');
  if (!content) return null;
  const match = content.match(/^version\s*=\s*"([^"]+)"/m);
  return match?.[1] ?? null;
}
