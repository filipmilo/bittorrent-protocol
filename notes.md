# Peer Protocol


- Connections are symmetrical. Messages look the same in both directions
- When a file is downloaded the piece hash is checked to validate that the file is downloaded after that the peer announces to other peers
- Connection contain two bits (choking, interest)
- Chocking is a notification that no data will be sent
- Data transfer only if one side is **interested** and other is **not choking**
- Interested: "Does a peer have piece i need?"
- Choked: "Are they willing to send data"
- All connections start out choked and not interested
- If a peer does not have pieces my peer needs, send them 'NOT INTERESTED' even if my peer is 'CHOKED'
- If a peer has pieces my peer need, send them 'INTERESTED' even if my peer is 'CHOKED'


- We are using url percentage encoding on the info_hash field because of problematic urls
