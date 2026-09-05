+++
title = "Giữ MixEngine luôn mới"
slug = "updating"
order = 12
summary = "Cập nhật là do bạn chọn, được đối chiếu chữ ký, và được diễn thử trước khi thay bất cứ thứ gì — và có một chương trình cố ý không bao giờ được thay theo đường này."
translation_of = "en/updating.md"
source_sha256 = "d7555eb599dc828c78f95492e52d26b9d536a2061604e993d407021d3717d62e"
+++

# Giữ MixEngine luôn mới

```bash
mix self-update --check
mix self-update
```

`--check` in ra thứ đang có — phiên bản, dung lượng, và những gì đã thay đổi — và không cài gì.
Không có nó thì cũng thông tin ấy được hiện ra rồi bạn được hỏi.

## Cập nhật không bao giờ diễn ra âm thầm

Một lần cập nhật khởi động lại những service bạn đang chạy. Điều đó khiến nó là việc bạn chọn, chứ
không phải việc xảy đến với bạn giữa lúc đang làm, nên **không gì được cài mà không hỏi**. Daemon có
kiểm tra âm thầm — lúc khởi động, và theo một nhịp mỗi ngày — để `mix status` có thể nói cho bạn
biết là có bản mới, và cả hai lần kiểm tra đều im lặng khi thất bại: một cái máy không có mạng không
phải một cái máy có vấn đề.

`--yes` trả lời câu hỏi trước, cho một script không có ai ngồi ở bàn phím.

## Chuyện gì xảy ra khi bạn đồng ý

Theo thứ tự, và không bước nào là tùy chọn:

1. Bản phát hành được tải về và băm để đối chiếu với feed cập nhật **đã ký**. Một gói không khớp thì
   không được giải nén.
2. Chữ ký được kiểm với một khóa công khai được biên dịch thẳng vào MixEngine. Không có gì thuộc về
   đường truyền được tin để quyết định một file có phải của chúng tôi hay không.
3. **`mixengined` mới được chạy thử một lần**, trước khi bất cứ thứ gì bị thay, để chắc rằng máy này
   sẽ khởi động được nó. Một bản cập nhật sẽ để lại cho bạn một daemon không chạy được thì bị chặn ở
   đây chứ không bị phát hiện sau đó.
4. Những gì đang chạy được dừng lại, các chương trình được thay, và daemon thoát.
5. `mix` khởi động daemon mới, và daemon mới khởi động lại các service của bạn.

## Chương trình duy nhất việc này không bao giờ đụng tới

`mixengine-elevate` chạy với quyền quản trị, và thay nó là một hành vi đặc quyền.
`mix self-update` cố ý để nó y nguyên.

```bash
mix elevation upgrade
```

Đó là hành động riêng, có chủ ý. Nó tải chương trình phụ trợ mà bản phát hành này công bố, kiểm chữ
ký của MixEngine trên đó, chạy thử một lần để chắc nó khởi động được, và đặt bản thay thế vào hàng
đợi. **Không có gì được cài bởi câu lệnh đó**: `mix elevation grant` mới là thứ bật hộp thoại lên,
và chương trình phụ trợ đang cài tự kiểm lại chữ ký một lần nữa trước khi cho phép bất cứ thứ gì ghi
đè lên nó.

Bản cũ và bản mới cùng tồn tại an toàn trong lúc đó. Daemon và chương trình phụ trợ thống nhất một
phiên bản giao thức khi chúng nói chuyện, và một chương trình phụ trợ cũ vẫn phục vụ những thao tác
nó biết trong khi MixEngine đề nghị bạn nâng cấp nó.

## Khi MixEngine được cài bằng trình quản lý gói

`mix self-update` từ chối, nói ra điều đó, và nêu tên thư mục. Đó là đúng chứ không phải vô ích: một
bản do `apt`, `dnf` hay một `.pkg` cài thuộc quyền quản lý của trình quản lý gói đó, và thay file
bên dưới nó sẽ để lại một hệ thống mà hồ sơ của chính nó mô tả một thứ không còn ở đó nữa. Hãy cập
nhật theo đúng cách bạn đã cài.

Bản zip xách tay, AppImage, bản cài Windows cho riêng một người dùng, và bản dựng từ mã nguồn đều
được `mix self-update` cập nhật bình thường.

## Phiên bản

MixEngine dùng semantic versioning, một phiên bản duy nhất cho mọi thứ nó phát hành. Trước 1.0, API
có thể phá vỡ tương thích giữa các phiên bản phụ, và mỗi lần như vậy đều được liệt kê trong
changelog — đó chính là thứ `mix self-update --check` in ra trước khi hỏi.
