"""The three publishers against in-memory fakes of Confluence and Azure DevOps (via `responses`)
and a local bare git repository standing in for a GitHub wiki."""

import importlib
import json
import re
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
from urllib.parse import parse_qs, urlparse

import pytest
import responses

import publish_azure_devops_wiki as ado
import publish_confluence as cf
import publish_github_wiki as gh
from wiki_common import load_tree


def run(module, argv, monkeypatch):
    monkeypatch.setattr(sys, "argv", [module.__file__, *argv])
    return module.main()


# ── Confluence ─────────────────────────────────────────────────────────────────────────────────


class FakeConfluence:
    BASE = "https://acme.atlassian.net/wiki/rest/api"

    def __init__(self):
        self.pages: dict[str, dict] = {}
        self.attachments: dict[str, dict[str, bytes]] = {}
        self.calls: list[str] = []
        self.n = 1000

    def _page(self, p):
        return {"id": p["id"], "title": p["title"], "version": dict(p["version"]),
                "ancestors": [{"id": a} for a in p["ancestors"]],
                "metadata": {"labels": {"results": [{"name": n} for n in p["labels"]]}}}

    def _chain(self, parent):
        out, cur = [], parent
        while cur:
            out.insert(0, cur)
            cur = self.pages[cur]["ancestors"][-1] if self.pages[cur]["ancestors"] else None
        return out

    def register(self, rs):
        b = re.escape(self.BASE)
        rs.add_callback("GET", re.compile(b + r"/content\?"), self.find)
        rs.add_callback("POST", re.compile(b + r"/content$"), self.create)
        rs.add_callback("PUT", re.compile(b + r"/content/\d+$"), self.update)
        rs.add_callback("POST", re.compile(b + r"/content/\d+/label$"), self.label)
        rs.add_callback("GET", re.compile(b + r"/content/\d+/child/attachment"), self.list_att)
        rs.add_callback("POST", re.compile(b + r"/content/\d+/child/attachment"), self.add_att)
        rs.add_callback("GET", re.compile(b + r"/content/search"), self.search)
        rs.add_callback("DELETE", re.compile(b + r"/content/\d+$"), self.delete)

    def find(self, req):
        q = parse_qs(urlparse(req.url).query)
        res = [self._page(p) for p in self.pages.values() if p["title"] == q["title"][0]]
        return 200, {}, json.dumps({"results": res})

    def create(self, req):
        d = json.loads(req.body)
        self.n += 1
        pid = str(self.n)
        parent = d.get("ancestors", [{}])[0].get("id") if d.get("ancestors") else None
        self.pages[pid] = {"id": pid, "title": d["title"], "body": d["body"]["storage"]["value"],
                           "version": {"number": 1, "message": d["version"]["message"]},
                           "ancestors": self._chain(parent) if parent and parent in self.pages else ([parent] if parent else []), "labels": []}
        self.calls.append(f"create {d['title']}")
        return 200, {}, json.dumps(self._page(self.pages[pid]))

    def update(self, req):
        pid = req.url.rsplit("/", 1)[1]
        d = json.loads(req.body)
        p = self.pages[pid]
        assert d["version"]["number"] == p["version"]["number"] + 1, "version must increase by one"
        ET.fromstring('<r xmlns:ac="a" xmlns:ri="r">' + d["body"]["storage"]["value"] + "</r>")  # valid XHTML
        parent = d.get("ancestors", [{}])[0].get("id") if d.get("ancestors") else None
        p.update(title=d["title"], body=d["body"]["storage"]["value"], version=d["version"],
                 ancestors=self._chain(parent) if parent else [])
        self.calls.append(f"update {d['title']}")
        return 200, {}, json.dumps(self._page(p))

    def label(self, req):
        pid = req.url.split("/content/")[1].split("/")[0]
        for l in json.loads(req.body):
            if l["name"] not in self.pages[pid]["labels"]:
                self.pages[pid]["labels"].append(l["name"])
        return 200, {}, "{}"

    def list_att(self, req):
        pid = req.url.split("/content/")[1].split("/")[0]
        name = parse_qs(urlparse(req.url).query).get("filename", [""])[0]
        res = [{"id": f"att{n}", "title": n} for n in self.attachments.get(pid, {}) if n == name]
        return 200, {}, json.dumps({"results": res})

    def add_att(self, req):
        pid = req.url.split("/content/")[1].split("/")[0]
        assert req.headers.get("X-Atlassian-Token") == "nocheck"
        name = re.search(rb'filename="([^"]+)"', req.body).group(1).decode()
        self.attachments.setdefault(pid, {})[name] = b"x"
        self.calls.append(f"attach {name}")
        return 200, {}, "{}"

    def search(self, req):
        cql = parse_qs(urlparse(req.url).query)["cql"][0]
        root = re.search(r"ancestor = (\d+)", cql).group(1)
        res = [{"id": p["id"], "title": p["title"]} for p in self.pages.values() if root in p["ancestors"] and "ekos-docs" in p["labels"]]
        return 200, {}, json.dumps({"results": res})

    def delete(self, req):
        pid = req.url.rsplit("/", 1)[1]
        self.calls.append(f"delete {self.pages.pop(pid)['title']}")
        return 204, {}, ""


@pytest.fixture
def confluence_env(monkeypatch):
    monkeypatch.setenv("CONFLUENCE_URL", "https://acme.atlassian.net/wiki")
    monkeypatch.setenv("CONFLUENCE_USER", "me@acme.com")
    monkeypatch.setenv("CONFLUENCE_API_TOKEN", "t")
    monkeypatch.delenv("CONFLUENCE_PAT", raising=False)


def test_confluence_publishes_tree_then_is_idempotent_then_prunes(docs, confluence_env, monkeypatch):
    fake = FakeConfluence()
    n_pages = len(load_tree(docs).ordered())
    with responses.RequestsMock(assert_all_requests_are_fired=False) as rs:
        fake.register(rs)
        args = ["--docs", str(docs), "--space", "DATA", "--mermaid", "code"]
        assert run(cf, args, monkeypatch) == 0
        assert sum(c.startswith("create") for c in fake.calls) == n_pages
        assert all("ekos-docs" in p["labels"] for p in fake.pages.values())
        # hierarchy: a silver table page sits under Target › Silver
        by_title = {p["title"]: p for p in fake.pages.values()}
        chain = [fake.pages[a]["title"] for a in by_title["public__payment"]["ancestors"]]
        assert chain[-2:] == ["Target", "Silver"]
        # page links became Confluence links
        assert '<ri:page ri:content-title="public.payment"' in by_title["public__payment"]["body"]
        assert 'ac:name="code"' in by_title["Source ER diagram"]["body"]

        fake.calls.clear()
        assert run(cf, args, monkeypatch) == 0
        assert not [c for c in fake.calls if c.startswith(("create", "update"))], fake.calls

        (docs / "source/tables/public.city.md").unlink()
        fake.calls.clear()
        assert run(cf, args + ["--prune"], monkeypatch) == 0
        assert "delete public.city" in fake.calls


def test_confluence_refuses_foreign_page_with_same_title(docs, confluence_env, monkeypatch):
    fake = FakeConfluence()
    fake.pages["1"] = {"id": "1", "title": "Gold", "body": "", "version": {"number": 3, "message": ""}, "ancestors": [], "labels": []}
    with responses.RequestsMock(assert_all_requests_are_fired=False) as rs:
        fake.register(rs)
        with pytest.raises(SystemExit, match="not published by this tool"):
            run(cf, ["--docs", str(docs), "--space", "DATA", "--mermaid", "code"], monkeypatch)


@pytest.mark.skipif(shutil.which("mmdc") is None, reason="mermaid-cli not installed")
def test_confluence_attaches_rendered_diagrams(docs, confluence_env, monkeypatch):
    fake = FakeConfluence()
    with responses.RequestsMock(assert_all_requests_are_fired=False) as rs:
        fake.register(rs)
        assert run(cf, ["--docs", str(docs), "--space", "DATA"], monkeypatch) == 0
    er = next(p for p in fake.pages.values() if p["title"] == "Source ER diagram")
    name = re.search(r'ri:filename="(mermaid-\w+\.png)"', er["body"]).group(1)
    assert name in fake.attachments[er["id"]]


# ── Azure DevOps ───────────────────────────────────────────────────────────────────────────────


class FakeAzureWiki:
    BASE = "https://dev.azure.com/acme/Data/_apis/wiki/wikis/Data.wiki"

    def __init__(self):
        self.pages: dict[str, dict] = {}
        self.attachments: set[str] = set()
        self.calls: list[str] = []
        self.etag = 0

    def register(self, rs):
        b = re.escape(self.BASE)
        rs.add_callback("GET", re.compile(b + r"/pages\?"), self.get)
        rs.add_callback("PUT", re.compile(b + r"/pages\?"), self.put)
        rs.add_callback("DELETE", re.compile(b + r"/pages\?"), self.delete)
        rs.add_callback("PUT", re.compile(b + r"/attachments\?"), self.attach)

    def _q(self, req):
        return {k: v[0] for k, v in parse_qs(urlparse(req.url).query).items()}

    def get(self, req):
        q = self._q(req)
        path = q["path"]
        if path not in self.pages:
            return 404, {}, "{}"
        if q.get("recursionLevel") == "full":
            def node(p):
                kids = [k for k in self.pages if k.startswith(p + "/") and "/" not in k[len(p) + 1:]]
                return {"path": p, "subPages": [node(k) for k in kids]}
            return 200, {}, json.dumps(node(path))
        p = self.pages[path]
        return 200, {"ETag": p["etag"]}, json.dumps({"path": path, "content": p["content"]})

    def put(self, req):
        q = self._q(req)
        path = q["path"]
        parent = path.rsplit("/", 1)[0]
        if parent and parent not in self.pages and parent != "/Lakehouse":
            return 404, {}, json.dumps({"message": f"parent {parent} missing"})
        if path in self.pages:
            assert req.headers.get("If-Match") == self.pages[path]["etag"], "update needs the ETag"
            self.calls.append(f"update {path}")
        else:
            assert "If-Match" not in req.headers
            self.calls.append(f"create {path}")
        self.etag += 1
        self.pages[path] = {"content": json.loads(req.body)["content"], "etag": f'"{self.etag}"'}
        return 200, {"ETag": f'"{self.etag}"'}, "{}"

    def delete(self, req):
        path = self._q(req)["path"]
        self.pages.pop(path)
        self.calls.append(f"delete {path}")
        return 200, {}, "{}"

    def attach(self, req):
        name = self._q(req)["name"]
        if name in self.attachments:
            return 409, {}, "already exists"
        self.attachments.add(name)
        return 201, {}, json.dumps({"path": f"/.attachments/{name}"})


@pytest.fixture
def ado_env(monkeypatch):
    for k, v in {"AZDO_ORG_URL": "https://dev.azure.com/acme", "AZDO_PROJECT": "Data", "AZDO_WIKI": "Data.wiki", "AZDO_PAT": "p"}.items():
        monkeypatch.setenv(k, v)
    monkeypatch.delenv("AZDO_WIKI_BRANCH", raising=False)


def test_azure_devops_publishes_tree_then_is_idempotent_then_prunes(docs, ado_env, monkeypatch):
    importlib.reload(ado)
    fake = FakeAzureWiki()
    fake.pages["/Lakehouse"] = {"content": "", "etag": '"0"'}
    with responses.RequestsMock(assert_all_requests_are_fired=False) as rs:
        fake.register(rs)
        args = ["--docs", str(docs), "--root-path", "/Lakehouse/Pagila"]
        assert run(ado, args, monkeypatch) == 0
        assert "/Lakehouse/Pagila/Target/Silver/public__payment" in fake.pages
        er = fake.pages["/Lakehouse/Pagila/Source/Source ER diagram"]["content"]
        assert "::: mermaid\nerDiagram" in er and "```mermaid" not in er
        pay = fake.pages["/Lakehouse/Pagila/Target/Silver/public__payment"]["content"]
        assert "](/Lakehouse/Pagila/Source/public.payment)" in pay
        assert "ekos:manual" not in pay

        fake.calls.clear()
        assert run(ado, args, monkeypatch) == 0
        assert fake.calls == []

        (docs / "source/tables/public.city.md").unlink()
        assert run(ado, args + ["--prune"], monkeypatch) == 0
        assert "delete /Lakehouse/Pagila/Source/public.city" in fake.calls


def test_azure_devops_link_paths_encode_hyphens_and_spaces():
    assert ado.link_path("/Lakehouse/Data quality & reconciliation/Code-quality") == "/Lakehouse/Data-quality-%26-reconciliation/Code%2Dquality"


# ── GitHub wiki ────────────────────────────────────────────────────────────────────────────────


def _bare_wiki(tmp_path):
    remote = tmp_path / "wiki.git"
    subprocess.run(["git", "init", "-q", "--bare", "-b", "master", str(remote)], check=True)
    seed = tmp_path / "seed"
    subprocess.run(["git", "clone", "-q", str(remote), str(seed)], check=True, capture_output=True)
    (seed / "Home.md").write_text("Welcome\n")
    for a in (["add", "-A"], ["-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "init"], ["push", "-q", "origin", "HEAD"]):
        subprocess.run(["git", *a], cwd=seed, check=True, capture_output=True)
    return remote


def _ls(remote):
    return subprocess.run(["git", "--git-dir", str(remote), "ls-tree", "--name-only", "HEAD"], capture_output=True, text=True).stdout.split()


def _log(remote):
    return subprocess.run(["git", "--git-dir", str(remote), "log", "--oneline"], capture_output=True, text=True).stdout.splitlines()


def test_github_wiki_refuses_to_clobber_a_hand_written_home(docs, tmp_path, monkeypatch):
    remote = _bare_wiki(tmp_path)
    with pytest.raises(SystemExit, match="refusing to overwrite"):
        run(gh, ["--docs", str(docs), "--remote", str(remote)], monkeypatch)


def test_github_wiki_flat_pages_sidebar_idempotent_prune(docs, tmp_path, monkeypatch):
    remote = _bare_wiki(tmp_path)
    args = ["--docs", str(docs), "--remote", str(remote), "--prefix", "Pagila"]
    assert run(gh, args, monkeypatch) == 0
    files = _ls(remote)
    assert {"Pagila.md", "_Sidebar.md", "Pagila-public__payment.md", "Pagila-Source-ER-diagram.md", "Home.md"} <= set(files)
    sidebar = subprocess.run(["git", "--git-dir", str(remote), "show", "HEAD:_Sidebar.md"], capture_output=True, text=True).stdout
    assert "    - [public__payment](Pagila-public__payment)" in sidebar
    page = subprocess.run(["git", "--git-dir", str(remote), "show", "HEAD:Pagila-public__payment.md"], capture_output=True, text=True).stdout
    assert "](Pagila-public.payment)" in page and "```mermaid" not in page
    commits = len(_log(remote))
    assert run(gh, args, monkeypatch) == 0
    assert len(_log(remote)) == commits, "no commit when nothing changed"
    (docs / "source/tables/public.city.md").unlink()
    assert run(gh, args + ["--prune"], monkeypatch) == 0
    assert not any("public.city" in f for f in _ls(remote))
    assert "Home.md" in _ls(remote), "pages the tool did not write are never pruned"
