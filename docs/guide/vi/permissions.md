+++
title = "MixEngine xin quyền để làm gì"
slug = "permissions"
order = 11
summary = "Mọi hộp thoại quyền quản trị mà MixEngine có thể bật lên, mỗi cái thay đổi chính xác điều gì, và vì sao không có gì của MixEngine ở lại máy với quyền root."
translation_of = "en/permissions.md"
source_sha256 = "d8cf1e1b88adc6ea28186c57a4d5dac00231b75deb86b98962e937e051b4fb51"
+++

# MixEngine xin quyền để làm gì

Một môi trường phát triển web trên máy cá nhân buộc phải đụng tới vài thứ thuộc về cả cỗ máy: cái
tên `blog.test` phải phân giải được, trình duyệt phải tin một chứng chỉ, phải có thứ gì đó lắng nghe
trên cổng 80. Quy tắc của MixEngine về tất cả những việc đó rất ngắn.

**Không thứ gì MixEngine chạy ở lại máy bạn với quyền quản trị.** Không phải daemon, không phải máy
chủ web, không phải cơ sở dữ liệu. Khi thật sự cần một thay đổi đặc quyền, một chương trình nhỏ tách
riêng tên là `mixengine-elevate` được khởi động qua chính hộp thoại của hệ điều hành, thực hiện đúng
thay đổi đó, rồi thoát. Nó không bao giờ chạy một câu lệnh ai đó đưa cho; nó biết đúng một nhúm thao
tác mà nó được phép làm, và tự kiểm từng yêu cầu thay vì tin daemon đã gửi yêu cầu đó.

## Các hộp thoại, và mỗi cái thay đổi gì

Có sáu, và bình thường bạn sẽ gặp chúng đúng một lần.

### Định tuyến các tên miền

Để `blog.test` và mọi thứ nằm dưới nó trỏ về chính máy bạn. MixEngine chạy một máy chủ DNS nhỏ trả
lời `127.0.0.1` cho mọi tên dưới một hậu tố được quản lý, và thứ cần xin phép là việc trỏ hệ thống
của bạn về nó — một file trong `/etc/resolver/` trên macOS, một luật resolver trên Linux, một luật
NRPT trên Windows.

Việc này được hỏi **một lần**, không phải mỗi site một lần, và đó chính là toàn bộ lý do máy chủ DNS
tồn tại: cách làm bằng cách sửa file hosts sẽ cần mật khẩu của bạn lại mỗi lần bạn tạo một site. Ở
nơi không dùng được đường resolver, MixEngine lùi về một dòng chính xác cho mỗi tên trong file
hosts, bên trong một khối được đánh dấu rõ ràng mà nó sở hữu và có thể gỡ đi.

### Tin chứng thực số

MixEngine tự phát hành chứng chỉ để các site của bạn là `https://` mà không có cảnh báo. Để trình
duyệt chấp nhận chúng, cái chứng thực số đã ký chúng phải nằm trong kho tin cậy của hệ thống, và đặt
nó vào đó thì cần quyền.

**Điều này có nghĩa và không có nghĩa gì.** Chứng thực số được sinh ra trên máy bạn và khóa riêng
của nó không bao giờ rời khỏi đó. Nó có thể bảo lãnh cho bất kỳ tên nào, nên cũng nên hiểu rằng cài
nó vào là một quyết định tin cậy thật sự — đúng cái quyết định mà mọi công cụ HTTPS cục bộ đều xin.
Từ chối là một câu trả lời hợp lệ: các site của bạn vẫn chạy qua `http://`, và MixEngine nói thẳng
ra điều đó thay vì báo lỗi.

Trên Linux, Chrome và Firefox đọc kho chứng chỉ riêng của chúng chứ không đọc kho hệ thống, nên
MixEngine ghi vào đó nữa — và việc này không cần quyền quản trị nào cả, vì những file đó là của bạn.

### Lắng nghe trên cổng 80 và 443

Trên macOS và Linux, các cổng dưới 1024 là đặc quyền. MixEngine không giải quyết chuyện này bằng
cách chạy máy chủ web với quyền root — nó cấp khả năng đó cho đúng một chương trình cần, và không
cho gì khác, rồi máy chủ chạy với quyền của bạn.

### Một luật tường lửa, khi bạn chia sẻ một site

Chỉ khi bạn muốn một site có thể truy cập được từ điện thoại hoặc từ máy khác trong cùng mạng. Luật
đó dành cho đúng cổng ấy, nó bị gỡ khi bạn ngừng chia sẻ, và không có gì khác trong tường lửa của
bạn bị đụng tới.

### Cài chương trình phụ trợ đặc quyền

Bản thân `mixengine-elevate` phải nằm ở nơi bạn không ghi được — một chương trình chạy với quyền
quản trị mà lại nằm trong thư mục tiến trình nào cũng ghi đè được thì không phải một ranh giới an
toàn. Nên việc đặc quyền đầu tiên mà MixEngine từng làm là đặt chương trình phụ trợ đó vào chỗ. Bốn
trong số các cách cài MixEngine chạy hoàn toàn với quyền của bạn (bản cài Windows, bản zip xách tay,
AppImage, và dựng từ mã nguồn), nên đây không thể là việc của bản cài. Ở nơi một gói `.deb`, `.rpm`
hay `.pkg` đã đặt sẵn nó, MixEngine nhận ra và không hỏi gì cả.

### Thay chương trình phụ trợ đặc quyền

Cập nhật không bao giờ đụng vào nó. `mix self-update` thay daemon và chương trình khách, và cố ý để
`mixengine-elevate` y nguyên; `mix elevation upgrade` là hành động riêng, có chủ ý, để lấy bản mới —
và chính chương trình phụ trợ đang cài kiểm chữ ký của MixEngine trên bản thay thế trước khi nó cho
phép ghi đè lên chính mình.

## Một hộp thoại, không phải sáu

MixEngine gom những việc cần quyền lại và hỏi một lần. Trên một máy mới tinh, tạo site HTTPS đầu
tiên thường có nghĩa là đúng một hộp thoại, bao trọn luật resolver, chứng thực số và quyền cổng.

Bạn có thể xem hàng đợi trước khi bất cứ gì được hỏi:

```bash
mix elevation status
```

Lệnh đó in ra mọi thao tác đang chờ và nó sẽ thay đổi cái gì — chính xác những dòng hosts, cổng nào,
kho nào. Rồi khi bạn sẵn sàng:

```bash
mix elevation grant
```

**Nói không là một câu trả lời bình thường.** Hàng đợi nằm nguyên tại chỗ, không có gì bị áp dụng dở
dang, và bạn có thể chạy lại lệnh này sau. Nếu bạn quyết định rằng một thao tác không bao giờ nên
được hỏi lại nữa:

```bash
mix elevation drop <id>
```

## Dấu vết kiểm toán

Chương trình phụ trợ ghi một dòng cho mỗi thao tác đặc quyền nó thực hiện, vào một file mà chỉ quyền
quản trị mới sửa được:

| Hệ điều hành | Đường dẫn |
| --- | --- |
| Windows | `%ProgramData%\MixEngine\elevate.log` |
| macOS | `/Library/Logs/MixEngine/elevate.log` |
| Linux | `/var/log/mixengine/elevate.log` |

File đó và bản thân chương trình phụ trợ là hai thứ duy nhất MixEngine để lại bên ngoài thư mục của
chính nó. `mix doctor` báo cáo cả hai và không gỡ cái nào — một công cụ chẩn đoán mà xóa mất dấu vết
kiểm toán thuộc quyền root thì đang xóa đúng cái ghi chép về thứ nó đang chẩn đoán. `mix uninstall`
mới là thứ gỡ chúng đi, và nó có hỏi.
