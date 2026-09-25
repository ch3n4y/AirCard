import { Button, Card, Flex, Select, Space, Tag, Typography } from "antd";
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

/**
 * 顶部：连着哪台 iPhone。
 *
 * 没设备时不出错而是说清楚要做什么——「没插」和「插了但没信任」是两回事，但都是人
 * 可以自己解决的事。
 */
export function DeviceBar({
  devices,
  selected,
  problem,
  busy,
  onSelect,
  onRefresh,
}: Props) {
  const device = devices.find((item) => item.udid === selected) ?? null;

  return (
    <Card size="small">
      <Flex align="center" justify="space-between" gap={12} wrap>
        <Space size={12} align="center">
          <Typography.Text strong>{t.selectedDevice}</Typography.Text>
          {devices.length === 0 ? (
            <Typography.Text type="secondary">{t.noDevice}</Typography.Text>
          ) : devices.length === 1 && device ? (
            <Space size={8} align="center">
              <Typography.Text strong>{device.name || t.unknownDevice}</Typography.Text>
              <Tag color={device.connection === "usb" ? "green" : "blue"}>
                {device.connection === "usb" ? t.usb : t.network}
              </Tag>
              <Typography.Text type="secondary">
                {device.product} · iOS {device.version} ({device.build})
              </Typography.Text>
            </Space>
          ) : (
            <Select
              value={selected ?? undefined}
              style={{ minWidth: 320 }}
              placeholder={t.connectHint}
              onChange={onSelect}
              options={devices.map((item) => ({
                value: item.udid,
                label: `${item.name || t.unknownDevice} · ${item.product} · iOS ${item.version}`,
              }))}
            />
          )}
          {devices.length > 1 && (
            <Typography.Text type="secondary">
              {t.devicesConnected(devices.length)}
            </Typography.Text>
          )}
        </Space>

        <Button
          icon={<ReloadOutlined />}
          disabled={busy !== null}
          onClick={onRefresh}
        >
          {t.refreshDevices}
        </Button>
      </Flex>

      {problem && (
        <Typography.Text type="warning" style={{ display: "block", marginTop: 8 }}>
          {problem} {t.connectHint}
        </Typography.Text>
      )}
    </Card>
  );
}
