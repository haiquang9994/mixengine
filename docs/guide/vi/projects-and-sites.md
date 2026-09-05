+++
title = "Dự án và site"
slug = "projects-and-sites"
order = 4
summary = "Hai danh từ MixEngine dựng lên trên đó, mỗi cái sở hữu gì, và làm sao một bản checkout tự mang theo cấu hình của nó."
translation_of = "en/projects-and-sites.md"
source_sha256 = "2f97de6e64cc9825a9775eff773507c07bf517f6fe246b75e0e40949b130cfc4"
+++

# Dự án và site

MixEngine có hai danh từ, và nên phân biệt rõ chúng.

**Project** là một thư mục trên đĩa của bạn mà MixEngine biết tới. Nó sở hữu đường dẫn, một cái tên,
và những phiên bản ngôn ngữ mà thư mục đó dùng.

**Site** là một thứ được phục vụ, nằm dưới một project. Nó sở hữu một hoặc nhiều tên miền, thứ gì
được phục vụ ra từ thư mục nào, và cái gì phục vụ nó. Một project không có site nào là chuyện hoàn
toàn bình thường — đó là một thư mục mà MixEngine biết phiên bản PHP của nó. Một project có thể có
nhiều site.

## Đăng ký một project

```bash
cd ~/code/blog
mix project create
mix project list
mix project show blog
```

Không tham số thì `mix project create` lấy thư mục hiện tại và đặt tên project theo thư mục đó.
`--name` ghi đè điều đó, còn `--pin` cố định một phiên bản ngôn ngữ cho mọi thứ nằm dưới thư mục ấy:

```bash
mix project create --name blog --pin php=^8.3 --pin node=22
```

`mix project update` đổi bất cứ thứ gì trong số đó về sau. Có một điều cần biết: `--pin` **thay
thế** toàn bộ tập pin chứ không thêm vào, và `--clear-pins` khi không có `--pin` nào thì xóa sạch.
Xóa một project là quên nó đi; các file của bạn vẫn y nguyên.

## Khai báo một site

```bash
mix site create --domain blog.test --kind php-fpm --https true
mix site list
mix site show blog.test
```

`--doc-root` là thư mục được phục vụ, tính tương đối so với gốc project — thường là `public` với các
framework PHP hiện đại, và là chính gốc project khi để trống. `--domain` có thể đưa nhiều lần; cái
đầu tiên là **tên chính**, các cái sau là bí danh. Tên chính có ý nghĩa: URL chuẩn của site và chứng
chỉ của nó được đặt theo tên đó.

`mix site update` thay đổi một site. Giống `--pin` ở trên, `--domain` và `--service` thay thế những
gì site đang có chứ không thêm vào — không đưa cái nào thì không đổi cái nào.

Bật và tắt một site là một cờ và một lần sinh lại cấu hình, không phải một tiến trình:

```bash
mix site stop blog.test
mix site start blog.test
```

Không có gì được khởi động hay bị giết bởi hai lệnh đó. Site là một lời khai báo; các service mà nó
dùng có trạng thái riêng của chúng.

## Bốn loại site

| `--kind` | Nó là gì |
| --- | --- |
| `php-fpm` | PHP, qua một pool của phiên bản mà thư mục này phân giải ra |
| `static` | File, và không có gì chạy cả |
| `reverse-proxy` | Chuyển tiếp toàn bộ tới một địa chỉ bạn đã có sẵn — `--upstream` |
| `node-app` | Một tiến trình Node bạn tự chạy, trên một cổng — `--port` |

`reverse-proxy` và `node-app` là hai loại đáng chú ý khi bạn đã có sẵn thứ gì đó đang chạy.
MixEngine cho nó một cái tên thật và một chứng chỉ mà không giành lấy việc khởi động nó.

## `mixengine.toml`, và việc nhận một bản checkout của đồng nghiệp

Một project có thể tự mô tả nó, trong một file được commit vào kho mã:

```toml
[project]
name = "blog"

[runtimes]
php = "8.3"
node = "22"

[site]
domain = "blog.test"
aliases = ["api.blog.test"]
doc_root = "public"
kind = "php-fpm"
https = true

[[services]]
name = "mariadb"
version = "11.4"
database = "blog"
```

Khi có file đó, `mix project create` rồi `mix site create` không tham số nào sẽ làm đúng những gì
file nói. Đó là hình dáng của việc nhận một bản checkout của người khác: clone, hai câu lệnh, và bạn
có cùng phiên bản PHP và cùng tên miền với người đã viết nó.

Theo chiều ngược lại, `mix project export` ghi project hiện tại vào `<root>/mixengine.toml`, giữ
nguyên mọi thứ khác đã có trong file.

## Thư mục này dùng phiên bản nào?

Bốn thứ có thể quyết định, và chúng được xét theo thứ tự này:

1. Một cờ hoặc biến môi trường tường minh cho câu lệnh bạn đang chạy.
2. File `mixengine.toml` gần nhất **có nhắc tới ngôn ngữ này**, đi ngược lên từ chỗ bạn đang đứng.
   Một manifest không nói gì về PHP thì không phải là câu trả lời về PHP, nên một pin ở ngoài vẫn có
   hiệu lực.
3. Project đã đăng ký có gốc là thư mục đó hoặc một thư mục cha của nó.
4. Mặc định toàn cục.

Thay vì tự suy ra, hãy hỏi:

```bash
mix runtime resolve php
```

Lệnh đó trả lời thư mục này nhận phiên bản đã cài nào **và cái nào trong bốn nguồn đã quyết định**,
đó mới là nửa mà người ta thật sự cần khi phiên bản làm họ bất ngờ. Không có gì được chạy để tìm ra
câu trả lời.

## Giữ ấm một project

Các service có thể bị dừng tự động khi đã lâu không ai dùng. Trong lúc bạn đang làm việc trên một
project, đó là một khoảng nghỉ bạn không muốn có:

```bash
mix project keep-warm blog
mix project keep-warm blog --off
```

Đây là một động từ riêng chứ không phải một thiết lập của project, vì nó là việc bạn làm trong một
buổi chiều chứ không phải một phần của việc project *là gì*. Nó với tới pool PHP mà các site của
project gọi tên; nó chưa với tới cơ sở dữ liệu mà chúng truy vấn, vì trong MixEngine không có gì ghi
lại project nào dùng cơ sở dữ liệu nào.
