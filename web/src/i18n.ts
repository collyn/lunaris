// Interface language. English is the default and the key: t("Some text")
// returns the translation for the chosen language, or the English text when
// there is none. Arguments: t("Connecting to %1…", name). The desktop app has
// the same scheme (client-qml/qml/I18n.qml).

export type Lang = 'en' | 'vi';

export const LANGUAGES: { value: Lang; label: string }[] = [
  { value: 'en', label: 'English' },
  { value: 'vi', label: 'Tiếng Việt' },
];

const readLang = (): Lang => {
  try {
    return localStorage.getItem('lunaris_lang') === 'vi' ? 'vi' : 'en';
  } catch {
    return 'en';
  }
};

let lang: Lang = readLang();

export const getLang = (): Lang => lang;

/** Saves the language and reloads the page so every text follows. */
export const setLang = (next: Lang) => {
  try { localStorage.setItem('lunaris_lang', next); } catch { /* private mode */ }
  lang = next;
  window.location.reload();
};

const vi: Record<string, string> = {
  // ---- stream settings ----
  'Stream settings': 'Cài đặt stream',
  'Close': 'Đóng',
  'Video': 'Video',
  'Remote computer': 'Máy được điều khiển',
  'Decoding': 'Giải mã',
  'Input': 'Điều khiển',
  'Picture': 'Hình ảnh',
  'Resolution': 'Độ phân giải',
  'Size of the picture the remote computer sends.': 'Kích thước hình ảnh máy kia gửi về.',
  'Frame rate': 'Tốc độ khung hình',
  "Frames per second sent by the remote computer. Above its screen's refresh rate needs a virtual screen (Remote computer).": 'Số khung hình mỗi giây máy kia gửi về. Cao hơn tần số quét màn hình của máy đó cần màn hình ảo (mục Máy được điều khiển).',
  'Bitrate': 'Bitrate',
  'Higher looks sharper but needs a faster network.': 'Cao hơn thì nét hơn nhưng cần mạng nhanh hơn.',
  'Compression': 'Nén hình',
  'Codec': 'Codec',
  'Auto: the best codec this browser and the remote computer both handle (AV1, then H.265, then H.264).': 'Tự động: codec tốt nhất mà trình duyệt này và máy kia cùng hỗ trợ (AV1, rồi H.265, rồi H.264).',
  'Chroma': 'Chroma',
  '4:4:4 keeps coloured text sharp. H.264 only, encoded on NVIDIA or the CPU.': '4:4:4 giữ chữ màu sắc nét. Chỉ H.264, máy kia mã hoá bằng NVIDIA hoặc CPU.',
  'Encoding': 'Mã hoá',
  'Encoder': 'Bộ mã hoá',
  "Auto picks the remote computer's hardware encoder (NVENC, VAAPI, QSV, AMF) and falls back to the CPU.": 'Tự động chọn bộ mã hoá phần cứng của máy kia (NVENC, VAAPI, QSV, AMF), không có thì dùng CPU.',
  'Screen': 'Màn hình',
  'Which screen of the remote computer to show.': 'Màn hình nào của máy kia được hiển thị.',
  'Virtual display': 'Màn hình ảo',
  'Create a separate screen on the remote computer for this session (Windows hosts with the virtual display driver).': 'Tạo một màn hình riêng trên máy kia cho phiên này (máy Windows có driver màn hình ảo).',
  'High-refresh virtual screen': 'Màn hình ảo tần số cao',
  'Linux hosts: turn on a forced high-refresh output so frame rates above the real screen are possible. Applies right away.': 'Máy Linux: bật một đầu ra tần số cao để stream được FPS cao hơn màn hình thật. Áp dụng ngay.',
  'Video decoding': 'Giải mã video',
  'Canvas renderer': 'Vẽ bằng canvas',
  'Draws frames on a canvas as soon as they are decoded, for lower latency. Not available in every browser.': 'Vẽ khung hình lên canvas ngay khi giải mã xong để giảm độ trễ. Không phải trình duyệt nào cũng hỗ trợ.',
  'Desktop app': 'Ứng dụng desktop',
  'Open sessions in the Lunaris desktop app instead of this browser.': 'Mở phiên bằng ứng dụng Lunaris trên máy tính thay vì trình duyệt này.',
  'Mouse and keyboard': 'Chuột và bàn phím',
  'Input protocol': 'Giao thức điều khiển',
  "WebTransport sends mouse and keyboard over QUIC datagrams, which lowers latency on lossy networks; WebRTC is used when it can't connect.": 'WebTransport gửi chuột và bàn phím qua QUIC datagram, giảm độ trễ khi mạng mất gói; nếu không kết nối được thì dùng WebRTC.',
  'Mouse queue limit': 'Giới hạn hàng đợi chuột',
  'How much mouse movement may wait to be sent. Smaller drops old moves for responsiveness; 256 B suits most networks.': 'Lượng chuyển động chuột được phép chờ gửi. Nhỏ hơn thì bỏ bớt chuyển động cũ để phản hồi nhanh; 256 B hợp với hầu hết mạng.',
  'Used when you connect to %1.': 'Dùng khi kết nối tới %1.',
  'Applies to this session.': 'Áp dụng cho phiên này.',
  'Display': 'Hiển thị',
  'Audio': 'Âm thanh',
  'Monitor': 'Màn hình',
  'Size of the picture the remote computer sends. Default 1080p (FHD).': 'Kích thước hình ảnh máy kia gửi về. Mặc định 1080p (FHD).',
  'Picture on screen': 'Kết xuất hình ảnh',
  'Display mode': 'Chế độ hiển thị',
  'How the remote screen fits this window. Applies right away.': 'Cách màn hình máy kia hiển thị trong cửa sổ này. Áp dụng ngay.',
  'Fit window': 'Vừa cửa sổ',
  'Fill window': 'Lấp đầy cửa sổ',
  'Stretch': 'Kéo giãn',
  'Original size': 'Kích thước gốc',
  'Statistics overlay': 'Hiện thông số',
  'Frame rate, bitrate, latency and the codec in a corner of the stream.': 'Tốc độ khung hình, bitrate, độ trễ và codec ở góc màn hình.',
  'Sound': 'Âm thanh',
  'Play sound': 'Phát âm thanh',
  "The remote computer's sound plays on this device. Applies right away.": 'Âm thanh của máy kia phát trên thiết bị này. Áp dụng ngay.',
  'Volume': 'Âm lượng',
  'Clipboard & files': 'Clipboard & file',
  'Clipboard': 'Clipboard',
  'Sync clipboard': 'Đồng bộ clipboard',
  'Text you copy here can be pasted on the remote computer, and the other way round. In the browser, paste with Ctrl+V in the stream; on http pages copied remote text arrives with your next key press or click.': 'Văn bản copy ở máy này dán được trên máy kia và ngược lại. Trên trình duyệt, dán bằng Ctrl+V trong màn hình stream; với trang http, nội dung copy từ máy kia sẽ vào clipboard ở lần bấm phím hoặc click tiếp theo.',
  'Files': 'File',
  'Send files': 'Gửi file',
  'Drop files on the stream or use the send button in the toolbar; they are saved in Downloads/Lunaris on the remote computer.': 'Kéo-thả file vào màn hình stream hoặc dùng nút gửi trên thanh công cụ; file được lưu vào Downloads/Lunaris trên máy kia.',
  'Send files to the remote computer (or drop them on the stream)': 'Gửi file tới máy kia (hoặc kéo-thả vào màn hình stream)',
  'Send files to the remote computer': 'Gửi file tới máy kia',
  'Drop to send to the remote computer (Downloads/Lunaris)': 'Thả để gửi tới máy kia (Downloads/Lunaris)',
  'File transfer is turned off in Settings': 'Gửi file đang tắt trong Cài đặt',
  'Connect to a computer to send files': 'Hãy kết nối tới một máy để gửi file',
  "Already sending files; try again when it's done": 'Đang gửi file; hãy thử lại khi gửi xong',
  'Sending %1: %2% of %3': 'Đang gửi %1: %2% / %3',
  'Sent %1 → %2': 'Đã gửi %1 → %2',
  'Sent %1 files': 'Đã gửi %1 file',
  'File transfer is turned off on this computer': 'Máy kia đã tắt nhận file',
  'Clipboard sharing is turned off on this computer': 'Máy kia đã tắt chia sẻ clipboard',
  'no answer from the remote computer': 'máy kia không phản hồi',
  'connection closed': 'mất kết nối',
  'Cancel': 'Huỷ',
  'Save': 'Lưu',
  'Apply': 'Áp dụng',
  'On': 'Bật',
  'Off': 'Tắt',
  'Auto': 'Tự động',
  'Default': 'Mặc định',
  'Auto (recommended)': 'Tự động (khuyên dùng)',
  'WebRTC data channels': 'WebRTC data channel',
  '(recommended)': '(khuyên dùng)',
  '(not supported by this browser)': '(trình duyệt này không hỗ trợ)',

  // ---- dashboard / sign-in ----
  'Language': 'Ngôn ngữ',
  'Devices': 'Thiết bị',
  'Users': 'Người dùng',
  'Groups': 'Nhóm',
  'TURN Servers': 'TURN server',
  'Device Directory': 'Danh sách máy',
  'Manage and connect to active remote agent streams': 'Quản lý và kết nối tới các máy đang hoạt động',
  'Sync Devices': 'Làm mới',
  'Connect': 'Kết nối',
  'IP Address': 'Địa chỉ IP',
  'Status': 'Trạng thái',
  'Online': 'Trực tuyến',
  'Offline': 'Ngoại tuyến',
  'Busy': 'Đang bận',
  'Stream settings for %1': 'Cài đặt stream cho %1',
  'Open in the Lunaris app': 'Mở bằng ứng dụng Lunaris',
  'Delete host': 'Xoá máy',
  'No agents connected': 'Chưa có máy nào kết nối',
  'There are no host agents registered with the server.': 'Chưa có máy nào đăng ký với server.',
  'Agents will appear here automatically once they connect.': 'Máy sẽ tự hiện ở đây khi kết nối.',
  'Add a host agent': 'Thêm máy',
  'Server URL and connection token': 'Địa chỉ server và token kết nối',
  'Signaling Tunnel': 'Qua server',
  'Sign in': 'Đăng nhập',
  'Sign out': 'Đăng xuất',
  'Username': 'Tên đăng nhập',
  'Password': 'Mật khẩu',
  'Server': 'Server',
  'Theme': 'Giao diện',
};

export const t = (text: string, ...args: (string | number)[]): string => {
  let out = (lang === 'vi' && vi[text]) || text;
  args.forEach((a, i) => { out = out.split(`%${i + 1}`).join(String(a)); });
  return out;
};
