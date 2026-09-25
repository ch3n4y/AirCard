import { Button, Select, Tooltip } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import type { DeviceInfo } from "../api";
import { t } from "../strings";

type Props = {
  devices: DeviceInfo[];
  selected: string | null;
  problem: string | null;
  busy: string | null;
  onSelect: (udid: string) => void;
  onRefresh: () => void;
};

export function DeviceBar({
  devices,
  selected,
  problem,
  busy,
  onSelect,
  onRefresh,
}: Props) {
  const device = devices.find((item) => item.udid === selected);
  return (
    <div className="device-panel">
      <div className="device-panel-label">{t.selectedDevice}</div>
      <div className="device-panel-row">
        <span className={"device-indicator" + (device ? " connected" : "")} />
        {devices.length > 1 ? (
          <Select
            className="device-selector"
            size="small"
            disabled={busy !== null}
            value={selected ?? undefined}
            onChange={onSelect}
            options={devices.map((item) => ({
              value: item.udid,
              label: item.name || t.unknownDevice,
            }))}
          />
        ) : (
          <strong className="device-name">{device?.name || t.noDevice}</strong>
        )}
        <Tooltip title={t.refreshDevices}>
          <Button
            type="text"
            size="small"
            icon={<ReloadOutlined />}
            disabled={busy !== null}
            onClick={onRefresh}
            aria-label={t.refreshDevices}
          />
        </Tooltip>
      </div>
      {device && (
        <span className="device-detail">
          {device.connection === "usb" ? t.usb : t.network} · {device.product} ·
          iOS {device.version}
        </span>
      )}
      {problem && (
        <span className="device-problem" title={problem}>
          {problem}
        </span>
      )}
    </div>
  );
}
