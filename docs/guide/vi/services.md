+++
title = "Máy chủ, cơ sở dữ liệu và bộ nhớ đệm"
slug = "services"
order = 6
summary = "Caddy hoặc Nginx, MariaDB, MySQL, PostgreSQL, Redis và Memcached — cài khi được yêu cầu, cấu hình sẵn cho bạn, và không bao giờ in mật khẩu ra."
translation_of = "en/services.md"
source_sha256 = "f703a461c2946bc8ce8609d276d859e3912a480cc80bd2d6cdcb125a62699179"
+++

# Máy chủ, cơ sở dữ liệu và bộ nhớ đệm

Hai từ, được phân biệt đúng như cách MixEngine phân biệt chúng.

**Package** là một chương trình MixEngine biết cách chạy — Caddy, MariaDB, Redis. Cài một package là
đặt một bản của nó vào thư mục riêng của MixEngine, và không làm gì khác.

**Service** là một thể hiện đang chạy của một package: một cổng, một thư mục dữ liệu, một cấu hình
được sinh ra, một file log, và một trạng thái. `mariadb@main` và `mariadb@legacy` là hai service của
cùng một package, với cổng khác nhau, dữ liệu khác nhau và có thể cả phiên bản khác nhau.

## Có sẵn những gì

| Service | Dòng mặc định | Cổng mặc định |
| --- | --- | --- |
| Caddy | 2.x | 80 và 443 — front end mặc định |
| Nginx | 1.27 | 80 và 443 — lựa chọn thay thế, mỗi lúc chỉ một front end |
| php-fpm | mỗi bản PHP đã cài một cái | một socket, hoặc một cổng cục bộ trên Windows |
| MariaDB | 11.4 LTS | 3306 |
| MySQL | 8.4 LTS | 3306 — một sản phẩm khác với MariaDB, không phải một phiên bản của nó |
| PostgreSQL | 16 | 5432 |
| Redis | 7.x | 6379 |
| Memcached | 1.6 | 11211 |

**Không có gì tự đến cả.** Một MixEngine mới tinh không có máy chủ web nào cho tới khi bạn cài một
cái, và chữ "mặc định" ở trên nghĩa là *cái mà dự án này khuyên dùng khi có lựa chọn*, chứ không
phải *cái đã có sẵn ở đó*.

## Cài và tạo

```bash
mix package available
mix package install mariadb 11.4.4
mix service create mariadb@main 11.4.4
```

Phần đứng trước `@` trong id của service là package mà nó là một thể hiện, và đó là lý do `mix
service create` không cần một tham số riêng cho nó. Phần sau `@` là của bạn: nó là thứ phân biệt hai
service, và MixEngine không gán ý nghĩa gì cho những chữ đó. Caddy chạy một lần cho cả một home của
MixEngine, nên service của nó chỉ đơn giản là `caddy`, không có `@` nào.

Id không đổi được về sau — nó cũng là thư mục cấu hình được sinh ra, thư mục log, file socket và địa
chỉ nơi mật khẩu được cất — nên đổi tên một service nghĩa là tạo cái kia và xóa cái này, và việc đó
giữ lại dữ liệu.

Vài cờ hữu ích của `mix service create`:

| Cờ | Nó làm gì |
| --- | --- |
| `--port` | Cổng nó lắng nghe. Mặc định của chính recipe khi để trống |
| `--bind` | Địa chỉ nó gắn vào. `127.0.0.1` khi để trống |
| `--data-dir` | Dữ liệu của nó nằm ở đâu. Một thư mục dưới home khi để trống |
| `--autostart` | Khởi động nó mỗi khi daemon khởi động |

### Ai được cổng 3306

MariaDB và MySQL cùng muốn một cổng, và hai thể hiện của bất kỳ cái nào cũng vậy. Quy tắc chỉ có
một: **ai tạo trước, người đó được trước**. Cái đầu tiên xin 3306 sẽ được nó; cái tiếp theo nhận
cổng trống đầu tiên phía trên. MixEngine báo lại cổng nó đã chọn, vì một cổng bạn không tự chọn là
một cổng phải được nói cho bạn biết.

Một cổng bạn nêu tường minh thì được nhận đúng như vậy, không có phân bổ nào cả.

### Một thư mục dữ liệu, một service

`mix service create` từ chối một `--data-dir` mà service khác đang giữ, và nêu tên ai đang giữ. Hai
máy chủ trên cùng một tập file sẽ làm hỏng chúng, và cái giá đó rơi vào dữ liệu của bạn chứ không
rơi vào một lần khởi động thất bại.

## Chạy chúng

```bash
mix service list
mix service status mariadb@main
mix service start mariadb@main
mix service stop mariadb@main
mix service logs mariadb@main --follow
```

`mix service status` bắt buộc có id trong khi `start` và những lệnh còn lại nhận id tùy chọn: một
`status` không có chủ ngữ là một `list` bị gõ sai, và trả lời nó như một `list` sẽ che mất điều đó.

Xóa một service lấy đi bản ghi và cấu hình sinh ra từ nó, và **không bao giờ lấy dữ liệu** — đó là
cơ sở dữ liệu của ai đó. Câu trả lời nêu tên thư mục đã được giữ lại, để không ai phải đi tìm:

```bash
mix service delete mariadb@legacy
```

## Cơ sở dữ liệu và tài khoản

Tạo một cơ sở dữ liệu là một câu lệnh, và nó khởi động máy chủ nếu máy chủ chưa chạy:

```bash
mix database create mariadb@main --name blog
mix database create mariadb@main --name shop --user shop_app
```

**Không có gì in mật khẩu ra.** Mật khẩu được sinh ra và đưa vào kho lưu thông tin đăng nhập của
chính hệ điều hành bạn — Credential Manager trên Windows, Keychain trên macOS, Secret Service trên
Linux — và thứ được in ra là địa chỉ nó đã được cất, dưới dạng tên kho và khóa của chính kho đó. Đó
là thứ cho phép một chương trình khách nói với bạn *"đã cất trong kho thông tin đăng nhập ở …"* mà
không ai phải mã hóa cứng cách đặt tên của MixEngine.

Để mở cơ sở dữ liệu bằng một chương trình quản trị trên máy:

```bash
mix database client mariadb@main   # có cái nào được cài, và hệ thống đã tìm ở đâu
mix database open mariadb@main     # mở nó
```

`client` chỉ đọc: nó không khởi động gì và không mở gì, và *"chưa cài chương trình nào"* là một câu
trả lời chứ không phải một lỗi — nó nêu tên nơi MixEngine đã tìm và nơi để tải về.

`open` khởi động thể hiện nếu nó đang dừng, đọc mật khẩu từ kho thông tin đăng nhập **ngay tại thời
điểm đó**, và trao nó cho chương trình khách trong môi trường của chính tiến trình ấy. Mật khẩu
không bao giờ được in ra, không bao giờ nằm trong một tham số, và vì thế không bao giờ nằm trong
lịch sử shell của bạn.

## Một service được lấy bao nhiêu, và khi nào thì nó dừng

```bash
mix service limits mariadb@main
mix service limits mariadb@main set --memory 512 --cpu 50
mix service idle mariadb@main --after 30m
```

`limits` không kèm lệnh con thì đọc; `set` thay thế; `clear` xóa. **`set` thay thế mọi trường, không
chỉ những trường bạn nêu** — `set --cpu 50` xóa mất trần bộ nhớ đang có — nên nó in ra cả ba trường
của kết quả, và một giới hạn vừa bị xóa nằm ngay trên màn hình bạn chứ không phải là một bất ngờ.
Thứ mà hệ điều hành của bạn thật sự cưỡng chế thì mỗi nơi mỗi khác, và câu trả lời nói rõ bạn đang
có cái nào: một trần **cứng** là một bức tường — chạm vào nó thì service bị giết hoặc lần cấp phát
tiếp theo thất bại — còn một trần **khuyến nghị** là một vạch được canh chừng mà service có thể vượt
qua, sau đó MixEngine cảnh báo và, ở nơi recipe cho phép, khởi động lại. Vẽ một điều khiển như một
lời bảo đảm trong khi nó chỉ là khuyến nghị thì là nói dối về dữ liệu của bạn.

`idle` cho biết khi nào một service bị dừng vì không ai dùng, và hiện thứ gì đang giữ nó mở. **Mặc
định không có gì tự dừng cả**: một service đã dừng thì nằm im cho tới khi bạn khởi động nó, nên bật
thứ này lên là một lựa chọn bạn đưa ra cho từng service.

## Cấu hình được sinh ra

MixEngine tự viết cấu hình cho mọi service nó chạy, từ những gì nó biết. Những file đó là thứ dùng
xong bỏ đi — chúng được sinh lại, không bao giờ được đọc ngược — nên ở đó không có gì để bạn sửa và
không có gì phải giữ cho khớp. Nếu một thiết lập bạn cần mà không có cờ nào tương ứng, đó là một
thiếu sót của MixEngine chứ không phải một lời mời sửa file.
