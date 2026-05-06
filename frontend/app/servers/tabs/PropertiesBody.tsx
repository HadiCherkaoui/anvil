"use client";

import {
	useState,
	type ChangeEvent,
	type ReactElement,
	type ReactNode,
} from "react";

import {
	ApiError,
	updateServerSettings,
	type Difficulty,
	type Gamemode,
	type ServerProperties,
} from "../../lib/api";
import { useServerDetail } from "../../lib/server-detail-context";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { SegmentedControl } from "../../components/SegmentedControl";
import { Tooltip } from "../../components/Tooltip";
import { useToast } from "../../components/Toast";

const DIFFICULTY_OPTIONS: ReadonlyArray<{ value: Difficulty; label: string }> =
	[
		{ value: "peaceful", label: "peaceful" },
		{ value: "easy", label: "easy" },
		{ value: "normal", label: "normal" },
		{ value: "hard", label: "hard" },
	];

const GAMEMODE_OPTIONS: ReadonlyArray<{ value: Gamemode; label: string }> = [
	{ value: "survival", label: "survival" },
	{ value: "creative", label: "creative" },
	{ value: "adventure", label: "adventure" },
	{ value: "spectator", label: "spectator" },
];

const TOGGLE_OPTIONS: ReadonlyArray<{ value: "off" | "on"; label: string }> = [
	{ value: "off", label: "off" },
	{ value: "on", label: "on" },
];

interface FieldHelp {
	tip: string;
	wikiAnchor: string;
}

const FIELD_HELP: Record<keyof ServerProperties, FieldHelp> = {
	difficulty: {
		tip: "controls hostile mob damage and spawning",
		wikiAnchor: "difficulty",
	},
	hardcore: {
		tip: "bans players on death; only meaningful from a fresh world",
		wikiAnchor: "hardcore",
	},
	gamemode: {
		tip: "default gamemode for new players",
		wikiAnchor: "gamemode",
	},
	force_gamemode: {
		tip: "forces all players back to the default gamemode on join",
		wikiAnchor: "force-gamemode",
	},
	max_players: {
		tip: "maximum concurrent players",
		wikiAnchor: "max-players",
	},
	view_distance: {
		tip: "chunks visible per player; 32 is max",
		wikiAnchor: "view-distance",
	},
	simulation_distance: {
		tip: "chunks ticking per player; usually <= view-distance",
		wikiAnchor: "simulation-distance",
	},
	pvp: {
		tip: "allow player-vs-player damage",
		wikiAnchor: "pvp",
	},
	white_list: {
		tip: "enforce whitelist; manage names in the players tab",
		wikiAnchor: "white-list",
	},
	spawn_protection: {
		tip: "blocks of spawn radius non-ops cannot modify; 0 disables",
		wikiAnchor: "spawn-protection",
	},
	spawn_animals: {
		tip: "passive mobs (cows, sheep, …) spawn",
		wikiAnchor: "spawn-animals",
	},
	spawn_monsters: {
		tip: "hostile mobs spawn",
		wikiAnchor: "spawn-monsters",
	},
	spawn_npcs: {
		tip: "villagers spawn",
		wikiAnchor: "spawn-npcs",
	},
	allow_flight: {
		tip: "lets clients fly (mods/creative); kicks otherwise",
		wikiAnchor: "allow-flight",
	},
	allow_nether: {
		tip: "permits nether portals",
		wikiAnchor: "allow-nether",
	},
	enable_command_block: {
		tip: "command blocks tickable by ops",
		wikiAnchor: "enable-command-block",
	},
	seed: {
		tip: "world seed; only meaningful from a fresh world. leave empty for random",
		wikiAnchor: "level-seed",
	},
};

const WIKI_BASE = "https://minecraft.wiki/w/Server.properties#";

function InfoLink({ field }: { field: keyof ServerProperties }): ReactElement {
	const help = FIELD_HELP[field];
	return (
		<Tooltip label={help.tip}>
			<a
				href={`${WIKI_BASE}${help.wikiAnchor}`}
				target="_blank"
				rel="noopener noreferrer"
				aria-label={`${field} on the minecraft wiki`}
				className="ml-1 inline-flex h-4 w-4 items-center justify-center rounded-full border border-border text-[10px] text-text-muted hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				i
			</a>
		</Tooltip>
	);
}

interface RowProps {
	field: keyof ServerProperties;
	label: string;
	children: ReactNode;
}

function Row({ field, label, children }: RowProps): ReactElement {
	return (
		<div className="flex items-center justify-between gap-3">
			<div className="flex items-center font-mono text-[11px] uppercase tracking-wider text-text-muted">
				<span>{label}</span>
				<InfoLink field={field} />
			</div>
			<div>{children}</div>
		</div>
	);
}

interface ToggleRowProps {
	field: keyof ServerProperties;
	label: string;
	value: boolean;
	onChange: (next: boolean) => void;
}

function ToggleRow({
	field,
	label,
	value,
	onChange,
}: ToggleRowProps): ReactElement {
	return (
		<Row field={field} label={label}>
			<SegmentedControl
				ariaLabel={label}
				value={value ? "on" : "off"}
				options={TOGGLE_OPTIONS}
				onChange={(v) => {
					onChange(v === "on");
				}}
			/>
		</Row>
	);
}

interface NumberRowProps {
	field: keyof ServerProperties;
	label: string;
	value: number;
	min: number;
	max: number;
	onChange: (next: number) => void;
}

function NumberRow({
	field,
	label,
	value,
	min,
	max,
	onChange,
}: NumberRowProps): ReactElement {
	return (
		<Row field={field} label={label}>
			<input
				type="number"
				min={min}
				max={max}
				value={value}
				onChange={(e: ChangeEvent<HTMLInputElement>) => {
					const n = Number.parseInt(e.target.value, 10);
					if (Number.isFinite(n)) onChange(n);
				}}
				className="w-20 rounded-md border border-border bg-bg px-2 py-1 text-right font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			/>
		</Row>
	);
}

interface TextRowProps {
	field: keyof ServerProperties;
	label: string;
	value: string;
	placeholder: string;
	maxLength: number;
	onChange: (next: string) => void;
}

function TextRow({
	field,
	label,
	value,
	placeholder,
	maxLength,
	onChange,
}: TextRowProps): ReactElement {
	return (
		<Row field={field} label={label}>
			<input
				type="text"
				value={value}
				placeholder={placeholder}
				maxLength={maxLength}
				spellCheck={false}
				autoComplete="off"
				onChange={(e: ChangeEvent<HTMLInputElement>) => {
					onChange(e.target.value);
				}}
				className="w-48 rounded-md border border-border bg-bg px-2 py-1 font-mono text-[12px] text-text-body placeholder:text-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			/>
		</Row>
	);
}

function shallowEqual(a: ServerProperties, b: ServerProperties): boolean {
	const keys = Object.keys(a) as Array<keyof ServerProperties>;
	return keys.every((k) => a[k] === b[k]);
}

export function PropertiesBody(): ReactElement {
	const { detail, refresh } = useServerDetail();
	const toast = useToast();
	const [props, setProps] = useState<ServerProperties>(detail.properties);
	const [busy, setBusy] = useState(false);
	const dirty = !shallowEqual(props, detail.properties);

	const set = <K extends keyof ServerProperties>(
		k: K,
		v: ServerProperties[K],
	): void => {
		setProps((p) => ({ ...p, [k]: v }));
	};

	const save = (): void => {
		setBusy(true);
		updateServerSettings(detail.id, { properties: props })
			.then(() => {
				toast.push("settings saved · applies on next start", "success");
				refresh();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`save failed · ${msg}`, "error");
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<div className="flex max-w-2xl flex-col gap-4">
			<Card header="world">
				<div className="flex flex-col gap-3">
					<Row field="difficulty" label="difficulty">
						<SegmentedControl
							ariaLabel="difficulty"
							value={props.difficulty}
							options={DIFFICULTY_OPTIONS}
							onChange={(v) => {
								set("difficulty", v);
							}}
						/>
					</Row>
					<Row field="gamemode" label="gamemode">
						<SegmentedControl
							ariaLabel="gamemode"
							value={props.gamemode}
							options={GAMEMODE_OPTIONS}
							onChange={(v) => {
								set("gamemode", v);
							}}
						/>
					</Row>
					<ToggleRow
						field="hardcore"
						label="hardcore"
						value={props.hardcore}
						onChange={(v) => {
							set("hardcore", v);
						}}
					/>
					<ToggleRow
						field="force_gamemode"
						label="force gamemode"
						value={props.force_gamemode}
						onChange={(v) => {
							set("force_gamemode", v);
						}}
					/>
					<TextRow
						field="seed"
						label="seed"
						value={props.seed}
						placeholder="empty = random"
						maxLength={256}
						onChange={(v) => {
							set("seed", v);
						}}
					/>
				</div>
			</Card>

			<Card header="players">
				<div className="flex flex-col gap-3">
					<NumberRow
						field="max_players"
						label="max players"
						value={props.max_players}
						min={1}
						max={200}
						onChange={(v) => {
							set("max_players", v);
						}}
					/>
					<NumberRow
						field="view_distance"
						label="view distance"
						value={props.view_distance}
						min={3}
						max={32}
						onChange={(v) => {
							set("view_distance", v);
						}}
					/>
					<NumberRow
						field="simulation_distance"
						label="simulation distance"
						value={props.simulation_distance}
						min={3}
						max={32}
						onChange={(v) => {
							set("simulation_distance", v);
						}}
					/>
					<ToggleRow
						field="pvp"
						label="pvp"
						value={props.pvp}
						onChange={(v) => {
							set("pvp", v);
						}}
					/>
					<ToggleRow
						field="white_list"
						label="whitelist enforced"
						value={props.white_list}
						onChange={(v) => {
							set("white_list", v);
						}}
					/>
				</div>
			</Card>

			<Card header="spawn">
				<div className="flex flex-col gap-3">
					<NumberRow
						field="spawn_protection"
						label="spawn protection"
						value={props.spawn_protection}
						min={0}
						max={256}
						onChange={(v) => {
							set("spawn_protection", v);
						}}
					/>
					<ToggleRow
						field="spawn_animals"
						label="spawn animals"
						value={props.spawn_animals}
						onChange={(v) => {
							set("spawn_animals", v);
						}}
					/>
					<ToggleRow
						field="spawn_monsters"
						label="spawn monsters"
						value={props.spawn_monsters}
						onChange={(v) => {
							set("spawn_monsters", v);
						}}
					/>
					<ToggleRow
						field="spawn_npcs"
						label="spawn npcs"
						value={props.spawn_npcs}
						onChange={(v) => {
							set("spawn_npcs", v);
						}}
					/>
				</div>
			</Card>

			<Card header="features">
				<div className="flex flex-col gap-3">
					<ToggleRow
						field="allow_flight"
						label="allow flight"
						value={props.allow_flight}
						onChange={(v) => {
							set("allow_flight", v);
						}}
					/>
					<ToggleRow
						field="allow_nether"
						label="allow nether"
						value={props.allow_nether}
						onChange={(v) => {
							set("allow_nether", v);
						}}
					/>
					<ToggleRow
						field="enable_command_block"
						label="command blocks"
						value={props.enable_command_block}
						onChange={(v) => {
							set("enable_command_block", v);
						}}
					/>
				</div>
			</Card>

			<div className="flex justify-end gap-2">
				<Button variant="primary" disabled={!dirty || busy} onClick={save}>
					save
				</Button>
			</div>
		</div>
	);
}
