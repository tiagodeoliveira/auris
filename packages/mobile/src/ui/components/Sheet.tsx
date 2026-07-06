import {
  Modal,
  Pressable,
  StyleSheet,
  Text,
  View,
  useWindowDimensions,
  type ModalProps,
} from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface SheetProps extends Pick<ModalProps, "transparent" | "animationType"> {
  visible: boolean;
  onClose: () => void;
  /** Title rendered in the sheet's grab-handle row. */
  title?: string;
  /** Slot at the right of the title row (e.g. Save / Done). */
  rightAction?: React.ReactNode;
  /** Children render inside the rounded-top sheet body. */
  children: React.ReactNode;
  /** Force a max height. Defaults to 80% of screen height. */
  maxHeight?: number;
}

export function Sheet({ visible, onClose, title, rightAction, children, maxHeight }: SheetProps) {
  const { height: windowH } = useWindowDimensions();
  const t = useTheme();
  const computedMax = maxHeight ?? windowH * 0.8;

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onClose}
      statusBarTranslucent
    >
      <Pressable style={styles.backdrop} onPress={onClose} accessibilityLabel="Close" />
      <View
        style={[
          styles.sheet,
          {
            backgroundColor: t.color.bg.elevated,
            paddingBottom: t.spacing.xl,
            ...t.shadow.floating,
          },
          { maxHeight: computedMax },
        ]}
        pointerEvents="box-none"
      >
        <View style={[styles.handleRow, { paddingVertical: t.spacing.sm }]}>
          <View
            style={{
              width: 40,
              height: 4,
              borderRadius: 2,
              backgroundColor: t.color.border.strong,
            }}
          />
        </View>
        <View
          style={[
            styles.titleRow,
            {
              paddingHorizontal: t.spacing.lg,
              paddingBottom: t.spacing.md,
              borderBottomColor: t.color.border.soft,
            },
          ]}
        >
          {title && (
            <Text style={{ ...t.type.subtitle, color: t.color.text.primary }} numberOfLines={1}>
              {title}
            </Text>
          )}
          <View style={styles.spacer} />
          {rightAction ?? (
            <Pressable onPress={onClose} hitSlop={8}>
              <Text
                style={{
                  ...t.type.body,
                  color: t.color.action.primary,
                  fontFamily: t.font.sansSemi,
                }}
              >
                Close
              </Text>
            </Pressable>
          )}
        </View>
        <View style={{ paddingHorizontal: t.spacing.lg, paddingTop: t.spacing.md }}>
          {children}
        </View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  backdrop: {
    ...StyleSheet.absoluteFillObject,
    backgroundColor: "rgba(0,0,0,0.35)",
  },
  sheet: {
    position: "absolute",
    bottom: 0,
    left: 0,
    right: 0,
    borderTopLeftRadius: 20,
    borderTopRightRadius: 20,
  },
  handleRow: {
    alignItems: "center",
  },
  titleRow: {
    flexDirection: "row",
    alignItems: "center",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  spacer: {
    flex: 1,
  },
});
