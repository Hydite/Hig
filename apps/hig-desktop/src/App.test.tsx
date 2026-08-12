import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { api } from "./api";

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return {
    ...actual,
    api: {
      bootstrap: vi.fn(),
      tasks: vi.fn(),
      projectStatus: vi.fn(),
      updateSettings: vi.fn(),
      unlockSession: vi.fn(),
      clearSession: vi.fn()
    }
  };
});

const snapshot = {
  version: "1.9.7",
  platform: "macos",
  daemonActive: true,
  sessionActive: false,
  settings: {
    defaultOutputDir: null,
    defaultSpeed: "balanced" as const,
    defaultEncryption: "password" as const,
    sessionTtlSecs: 1800,
    recentProjects: [],
    language: "en" as const,
    knownCacheDirs: []
  }
};

describe("Hig desktop", () => {
  afterEach(() => cleanup());

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.bootstrap).mockResolvedValue(snapshot);
    vi.mocked(api.tasks).mockResolvedValue([]);
  });

  it("opens on the operational project workspace", async () => {
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Projects" })).toBeInTheDocument();
    expect(screen.getByText("Secure defaults")).toBeInTheDocument();
    expect(screen.getByText("Daemon online")).toBeInTheDocument();
  });

  it("switches the desktop UI between English and Chinese", async () => {
    vi.mocked(api.updateSettings).mockImplementation(async (settings) => settings);
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Projects" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Language" }), { target: { value: "zh-CN" } });
    expect(await screen.findByRole("heading", { name: "设置" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));
    expect(api.updateSettings).toHaveBeenCalledWith(expect.objectContaining({ language: "zh-CN" }));
    expect(screen.getByText("安全默认")).toBeInTheDocument();
  });

  it("shows safe archive defaults and explicit fastest risk", async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByText("Secure defaults")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Create archive" }));
    expect(screen.getByText("Balanced")).toBeInTheDocument();
    expect(screen.getByText("Password")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Speed mode" }), { target: { value: "Fastest" } });
    expect(screen.getByText(/trusts metadata/i)).toBeInTheDocument();
  });

  it("clears the session password after unlock", async () => {
    vi.mocked(api.unlockSession).mockResolvedValue({});
    render(<App />);
    await waitFor(() => expect(screen.getByText("Secure defaults")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /Session locked/i }));
    const password = screen.getByLabelText("Password");
    fireEvent.change(password, { target: { value: "temporary-password" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(api.unlockSession).toHaveBeenCalled());
    expect(screen.queryByDisplayValue("temporary-password")).not.toBeInTheDocument();
  });

  it("localizes known backend errors by code", async () => {
    vi.mocked(api.updateSettings).mockImplementation(async (settings) => settings);
    vi.mocked(api.unlockSession).mockRejectedValue({
      code: "daemon_unavailable",
      message: "raw backend daemon failure",
      recoverable: true
    });
    render(<App />);
    await waitFor(() => expect(screen.getByText("Secure defaults")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Language" }), { target: { value: "zh-CN" } });
    expect(await screen.findByRole("heading", { name: "设置" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /会话已锁定/i }));
    fireEvent.change(screen.getByLabelText("密码"), { target: { value: "temporary-password" } });
    fireEvent.click(screen.getByRole("button", { name: "解锁" }));
    expect(await screen.findByText(/Hig daemon 不可用/)).toBeInTheDocument();
    expect(screen.queryByDisplayValue("temporary-password")).not.toBeInTheDocument();
  });
});
