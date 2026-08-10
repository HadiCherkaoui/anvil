// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useState, type ReactElement } from "react";

import { AddToWhitelistDialog } from "../../components/AddToWhitelistDialog";
import { BroadcastDialog } from "../../components/BroadcastDialog";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import {
	PlayerActionDialog,
	type PlayerActionVariant,
} from "../../components/PlayerActionDialog";
import { PlayerActionMenu } from "../../components/PlayerActionMenu";
import { Skeleton } from "../../components/Skeleton";
import { useToast } from "../../components/Toast";
import {
	ApiError,
	startServer,
	type BanEntry,
	type BanIpEntry,
	type PlayerEvent,
	type PlayersResponse,
} from "../../lib/api";
import { useServerDetailCtx } from "../../lib/server-detail-context";
import { usePlayers, type PlayersStatus } from "../../lib/use-players";

export function PlayersBody(): ReactElement {
	const detail = useServerDetailCtx();
	const enabled = detail.status === "running";
	const { data, status, refresh } = usePlayers(detail.id, { enabled });
	const toast = useToast();

	const [broadcastOpen, setBroadcastOpen] = useState(false);
	const [addOpen, setAddOpen] = useState(false);
	const [actionVariant, setActionVariant] =
		useState<PlayerActionVariant | null>(null);

	if (!enabled) {
		const onStart = (): void => {
			void startServer(detail.id)
				.then(() => {
					toast.push("starting server", "success");
				})
				.catch((err: unknown) => {
					const detailMsg =
						err instanceof ApiError
							? `${err.code}: ${err.message}`
							: err instanceof Error
								? err.message
								: "unknown error";
					toast.push(`start failed: ${detailMsg}`, "error");
				});
		};
		return (
			<Card>
				<div className="flex flex-col items-start gap-3 font-mono text-[13px]">
					<p className="text-text-muted">
						server is stopped — start the server to manage players.
					</p>
					<Button variant="primary" onClick={onStart}>
						start server
					</Button>
				</div>
			</Card>
		);
	}

	if (status === "loading" && data === null) {
		return (
			<div className="flex flex-col gap-4">
				<Skeleton variant="block" />
				<Skeleton variant="block" />
				<Skeleton variant="block" />
				<Skeleton variant="block" />
			</div>
		);
	}

	const view: PlayersResponse = data ?? {
		online: { count: 0, max: 0, players: [] },
		whitelist: [],
		banlist: { players: [], ips: [] },
		history: [],
	};

	return (
		<div className="flex flex-col gap-4">
			<BroadcastBar
				onOpen={() => {
					setBroadcastOpen(true);
				}}
				status={status}
			/>
			<OnlinePlayersCard
				view={view.online}
				serverId={detail.id}
				openDialog={setActionVariant}
				onDone={refresh}
			/>
			<WhitelistCard
				names={view.whitelist}
				serverId={detail.id}
				openDialog={setActionVariant}
				onAdd={() => {
					setAddOpen(true);
				}}
				onDone={refresh}
			/>
			<BanlistCard
				view={view.banlist}
				serverId={detail.id}
				openDialog={setActionVariant}
				onDone={refresh}
			/>
			<RecentActivityCard events={view.history} />

			<BroadcastDialog
				open={broadcastOpen}
				onClose={() => {
					setBroadcastOpen(false);
				}}
				serverId={detail.id}
			/>
			<AddToWhitelistDialog
				open={addOpen}
				onClose={() => {
					setAddOpen(false);
				}}
				serverId={detail.id}
				onDone={refresh}
			/>
			<PlayerActionDialog
				open={actionVariant !== null}
				onClose={() => {
					setActionVariant(null);
				}}
				serverId={detail.id}
				variant={actionVariant}
				onDone={refresh}
			/>
		</div>
	);
}

// ---- inline cards ----------------------------------------------------------

interface BroadcastBarProps {
	onOpen: () => void;
	status: PlayersStatus;
}

function BroadcastBar({ onOpen, status }: BroadcastBarProps): ReactElement {
	return (
		<div className="flex items-center justify-between font-mono text-[12px] text-text-muted">
			<Button variant="primary" onClick={onOpen}>
				broadcast
			</Button>
			<span>{status === "live" ? "live · 10s poll" : status}</span>
		</div>
	);
}

interface OnlinePlayersCardProps {
	view: PlayersResponse["online"];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onDone: () => void;
}

function OnlinePlayersCard({
	view,
	serverId,
	openDialog,
	onDone,
}: OnlinePlayersCardProps): ReactElement {
	return (
		<Card
			header={`online now · ${view.count.toString()} / ${view.max.toString()}`}
		>
			{view.players.length === 0 ? (
				<p className="font-mono text-[12px] text-text-dim">nobody online</p>
			) : (
				<ul className="divide-y divide-border-soft">
					{view.players.map((name) => (
						<li
							key={name}
							className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
						>
							<span>{name}</span>
							<PlayerActionMenu
								source="online"
								serverId={serverId}
								name={name}
								openDialog={openDialog}
								onDone={onDone}
							/>
						</li>
					))}
				</ul>
			)}
		</Card>
	);
}

interface WhitelistCardProps {
	names: readonly string[];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onAdd: () => void;
	onDone: () => void;
}

function WhitelistCard({
	names,
	serverId,
	openDialog,
	onAdd,
	onDone,
}: WhitelistCardProps): ReactElement {
	return (
		<Card header={`whitelist · ${names.length.toString()} names`}>
			{names.length === 0 ? (
				<p className="mb-3 font-mono text-[12px] text-text-dim">
					whitelist is empty (the server may not have whitelist enabled)
				</p>
			) : (
				<ul className="mb-3 divide-y divide-border-soft">
					{names.map((name) => (
						<li
							key={name}
							className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
						>
							<span>{name}</span>
							<PlayerActionMenu
								source="whitelist"
								serverId={serverId}
								name={name}
								openDialog={openDialog}
								onDone={onDone}
							/>
						</li>
					))}
				</ul>
			)}
			<Button variant="secondary" onClick={onAdd}>
				+ add
			</Button>
		</Card>
	);
}

interface BanlistCardProps {
	view: PlayersResponse["banlist"];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onDone: () => void;
}

function BanlistCard({
	view,
	serverId,
	openDialog,
	onDone,
}: BanlistCardProps): ReactElement {
	const total = view.players.length + view.ips.length;
	if (total === 0) {
		return (
			<Card header="banned · 0 players · 0 ips">
				<p className="font-mono text-[12px] text-text-dim">nobody banned</p>
			</Card>
		);
	}
	return (
		<Card
			header={`banned · ${view.players.length.toString()} players · ${view.ips.length.toString()} ips`}
		>
			{view.players.length > 0 && (
				<>
					<p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-text-faint">
						players
					</p>
					<ul className="mb-3 divide-y divide-border-soft">
						{view.players.map((b: BanEntry) => (
							<li
								key={b.name}
								className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
							>
								<span>
									<span className="text-text-primary">{b.name}</span>
									<span className="ml-2 text-text-muted">· {b.reason}</span>
								</span>
								<PlayerActionMenu
									source="banlist"
									serverId={serverId}
									name={b.name}
									openDialog={openDialog}
									onDone={onDone}
								/>
							</li>
						))}
					</ul>
				</>
			)}
			{view.ips.length > 0 && (
				<>
					<p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-text-faint">
						ips
					</p>
					<ul className="divide-y divide-border-soft">
						{view.ips.map((b: BanIpEntry) => (
							<li
								key={b.ip}
								className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
							>
								<span>
									<span className="text-text-primary">{b.ip}</span>
									<span className="ml-2 text-text-muted">· {b.reason}</span>
								</span>
								<PlayerActionMenu
									source="banlist"
									serverId={serverId}
									ip={b.ip}
									openDialog={openDialog}
									onDone={onDone}
								/>
							</li>
						))}
					</ul>
				</>
			)}
		</Card>
	);
}

interface RecentActivityCardProps {
	events: readonly PlayerEvent[];
}

function RecentActivityCard({ events }: RecentActivityCardProps): ReactElement {
	return (
		<Card header="recent activity">
			{events.length === 0 ? (
				<p className="font-mono text-[12px] text-text-dim">
					no recent join/leave events in pod logs
				</p>
			) : (
				<ul className="font-mono text-[12px] text-text-body">
					{events.map((ev) => (
						<li
							key={`${ev.player}-${ev.kind}-${ev.ts_ms.toString()}`}
							className="py-1"
						>
							<span className="text-text-primary">{ev.player}</span>{" "}
							<span
								className={
									ev.kind === "joined"
										? "text-state-running"
										: "text-text-muted"
								}
							>
								{ev.kind}
							</span>
							<span className="ml-2 text-text-dim">
								· {relativeTime(ev.ts_ms)}
							</span>
						</li>
					))}
				</ul>
			)}
		</Card>
	);
}

function relativeTime(tsMs: number): string {
	const diff = Math.max(0, Date.now() - tsMs);
	const sec = Math.floor(diff / 1000);
	if (sec < 60) return `${sec.toString()}s ago`;
	const min = Math.floor(sec / 60);
	if (min < 60) return `${min.toString()}m ago`;
	const hr = Math.floor(min / 60);
	if (hr < 24) return `${hr.toString()}h ago`;
	const day = Math.floor(hr / 24);
	return `${day.toString()}d ago`;
}
