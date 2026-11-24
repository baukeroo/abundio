import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
	add: vi.fn(),
	check: vi.fn(),
	list: vi.fn(),
	remove: vi.fn(),
	start: vi.fn(),
	install: vi.fn(),
	uninstall: vi.fn(),
	removeService: vi.fn(),
}));

vi.mock("../../../lib/ipc", () => ({
	hostsApi: api,
	listen: () => Promise.resolve(() => {}),
	listenToThisWindow: () => Promise.resolve(() => {}),
}));
vi.mock("../../../lib/hostProbeAgents", () => ({
	hostProbeCommands: () => [],
}));
vi.mock("../../../stores/hostAgentsStore", () => ({
	useHostAgentsStore: { getState: () => ({ settings: {} }) },
}));
vi.mock("../HostFootprintPopover", () => ({
	HostFootprintButton: () => null,
}));

import { HostsSection } from "../HostsSection";

(
	globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const flush = () =>
	act(async () => new Promise((resolve) => setTimeout(resolve, 0)));

describe("HostsSection Docker Hosts", () => {
	let container: HTMLDivElement;
	let root: Root;

	beforeEach(() => {
		vi.clearAllMocks();
		api.list.mockResolvedValue([]);
		api.add.mockResolvedValue({
			id: "docker-host",
			name: "Docker Host",
			target: { kind: "docker", container: "abundio-host-test" },
		});
		api.check.mockResolvedValue({
			probe: {
				os: "Linux",
				arch: "x86_64",
				user: "root",
				current: null,
				installed: [],
				running: false,
				panes: null,
				agents: [],
			},
			items: [],
			appVersion: "4.0.0",
		});
		container = document.createElement("div");
		document.body.appendChild(container);
		root = createRoot(container);
		act(() => root.render(<HostsSection />));
	});

	afterEach(() => {
		act(() => root.unmount());
		container.remove();
	});

	it("creates a Docker Host without an SSH destination", async () => {
		await flush();
		const dockerChoice = [...container.querySelectorAll("button")].find(
			(button) => button.textContent === "Docker container",
		);
		act(() => dockerChoice?.click());
		const create = [...container.querySelectorAll("button")].find(
			(button) => button.textContent === "Create and check",
		);
		expect(create?.disabled).toBe(false);
		act(() => create?.click());
		await flush();
		expect(api.add).toHaveBeenCalledWith("Docker Host", { kind: "docker" });
		expect(api.check).toHaveBeenCalledWith("docker-host", []);
	});
});
