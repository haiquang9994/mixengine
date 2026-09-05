+++
title = "Extension"
slug = "extensions"
order = 10
summary = "Những công cụ bạn hay dùng bên cạnh stack — phpMyAdmin, Mailpit, MinIO — cài từ một registry đã ký, và mỗi cái được phép làm gì đều hiện ra trước khi bạn đồng ý."
translation_of = "en/extensions.md"
source_sha256 = "a9de83194e3bc7b8021e35c615cabd3f9bdc17333e30672a29d884b58ce7acec"
+++

# Extension

Extension là một công cụ sống bên cạnh stack của bạn chứ không nằm trong nó: một giao diện quản trị
cơ sở dữ liệu, một cái bẫy email, một kho object, một máy tìm kiếm. MixEngine cài nó, giám sát nó,
và cho nó một cái tên cùng một chứng chỉ đúng như cách nó làm với các site của bạn.

## Có gì

```bash
mix extension available
mix extension list
```

`available` là registry đã ký mà MixEngine phát hành; `list` là những gì máy này đã cài.

Một extension có thể mang bốn hình dáng, và nên nhận ra mình đang cài loại nào:

| Loại | Nó là gì |
| --- | --- |
| `web-app` | Mã nguồn được phục vụ trên chính stack của bạn, ở một site nội bộ sinh ra — phpMyAdmin, Adminer |
| `service` | Một chương trình MixEngine giám sát như mọi service khác — Mailpit, MinIO, MeiliSearch |
| `desktop-app` | Một ứng dụng trên máy bạn mà MixEngine tìm ra và trao cho một kết nối |
| `recipe` | Chỉ cấu hình: thêm chỉ thị cho máy chủ web, một hồ sơ `php.ini` |

## Xem trước khi cài

```bash
mix extension plan mailpit
```

Lệnh này không thay đổi gì và in ra thứ mà việc cài sẽ tạo ra: nó sẽ tải gì, sẽ tạo service nào, sẽ
truy cập được ở site nào, và **nó đang xin được phép làm gì**.

Có hai dòng trong kế hoạch đó đáng đọc kỹ chứ không nên lướt, và chúng chỉ xuất hiện với một
`web-app`:

- **Một giao diện quản trị sẽ mở lên cơ sở dữ liệu nào.** Một công cụ như phpMyAdmin đóng băng điều
  đó ngay lúc cài, và việc nó quản trị máy chủ nào không phải là chi tiết nên phát hiện ra sau.
- **Nó sẽ đăng nhập bằng tài khoản nào.** Một extension có thể khai báo rằng nó đăng nhập bằng tài
  khoản quản trị cao nhất của một máy chủ — đó là thứ hệ trọng nhất mà một extension có thể được
  trao. Kế hoạch nêu tên tài khoản, nói rằng mật khẩu được lấy từ kho thông tin đăng nhập của hệ
  điều hành khi pool khởi động, và nói rằng không có gì ghi nó xuống đĩa.

`mix extension install` hỏi về tất cả những điều đó trước khi làm bất cứ việc gì. `--yes` bỏ qua câu
hỏi, và dành cho một script đã đọc kế hoạch rồi.

## Cài và gỡ

```bash
mix extension install mailpit
mix extension start mailpit
mix extension stop mailpit
mix extension uninstall mailpit
```

Cài là một job; `--no-wait` đưa cho bạn id job thay vì chờ.

`mix extension uninstall` **giữ lại dữ liệu của extension** trừ khi bạn nói khác, vì đó là câu trả
lời có thể hoàn tác. `--delete-data` là câu trả lời không thể.

## Cài thứ registry không có

```bash
mix extension inspect ./my-tool
mix extension plan --path ./my-tool
mix extension install --path ./my-tool
```

`inspect` đọc một `extension.toml` và cho bạn biết nó khai báo gì, mà không cài gì cả.

**Không gì bảo lãnh cho một extension cài từ đường dẫn**, và bản ghi nói rõ điều đó chừng nào nó còn
được cài. Đó không phải một cảnh báo bạn bấm bỏ qua được: nó là thứ khiến một extension chưa ký hiện
rõ trong mọi danh sách nêu tên nó, để không ai phải nhớ nó đến từ đâu.

## Extension không phải là gì

**Extension không phải một chương trình khách của API.** Nó không được gọi API của chính MixEngine,
không được nhờ daemon thay đổi máy bạn, và không với tới được thứ gì nó không khai báo. Thứ nó nhận
được là những gì manifest của nó đã khai báo và những gì bạn đã đồng ý — một cổng, một site, một
service, một kết nối cơ sở dữ liệu — và không gì khác.

Site của một extension xuất hiện trong `mix site list` như mọi site khác, và có thể bật tắt. Mọi
thay đổi khác lên nó đều bị từ chối, và lời từ chối nêu tên câu lệnh gỡ cài đặt sẽ xóa nó đi: site
đó thuộc về extension, và sửa nó từ bên dưới extension sẽ là một cách âm thầm làm hỏng extension.
